use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use clap::{Parser, Subcommand};
use serde::Serialize;
use sqlx::Row;
use televy_backup_core::bootstrap::PinnedStorage;
use televy_backup_core::{
    APP_NAME, BackupConfig, BackupOptions, ChunkingConfig, ProgressSink, RestoreConfig,
    RestoreOptions, Storage, TelegramMtProtoStorage, TelegramMtProtoStorageConfig, VerifyConfig,
    VerifyOptions, restore_snapshot_with, run_backup_with, verify_snapshot_with,
};
use televy_backup_core::{bootstrap, config_bundle};
use televy_backup_core::{config as settings_config, gold_key};
use tokio::io::AsyncBufReadExt;
#[cfg(unix)]
use tokio::net::UnixStream;

#[derive(Parser)]
#[command(name = "televybackup")]
#[command(about = "TelevyBackup CLI (native macOS app backend)", long_about = None)]
struct Cli {
    #[arg(long)]
    json: bool,

    #[arg(long)]
    events: bool,

    #[arg(long)]
    config_dir: Option<PathBuf>,

    #[arg(long)]
    data_dir: Option<PathBuf>,

    #[command(subcommand)]
    cmd: Command,
}

#[derive(Subcommand)]
enum Command {
    Ping {
        value: String,
    },
    Settings {
        #[command(subcommand)]
        cmd: SettingsCmd,
    },
    Status {
        #[command(subcommand)]
        cmd: StatusCmd,
    },
    Secrets {
        #[command(subcommand)]
        cmd: SecretsCmd,
    },
    Telegram {
        #[command(subcommand)]
        cmd: TelegramCmd,
    },
    Snapshots {
        #[command(subcommand)]
        cmd: SnapshotsCmd,
    },
    Stats {
        #[command(subcommand)]
        cmd: StatsCmd,
    },
    Backup {
        #[command(subcommand)]
        cmd: BackupCmd,
    },
    Restore {
        #[command(subcommand)]
        cmd: RestoreCmd,
    },
    Verify {
        #[command(subcommand)]
        cmd: VerifyCmd,
    },
}

#[derive(Subcommand)]
enum SettingsCmd {
    Get {
        #[arg(long)]
        with_secrets: bool,
    },
    Set,
    ExportBundle {
        #[arg(long)]
        hint: Option<String>,
    },
    ImportBundle {
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        apply: bool,
    },
}

#[derive(Subcommand)]
enum SecretsCmd {
    SetTelegramBotToken {
        #[arg(long)]
        endpoint_id: Option<String>,
    },
    SetTelegramApiHash,
    ClearTelegramMtprotoSession,
    MigrateKeychain,
    InitMasterKey,
    ExportMasterKey {
        #[arg(long)]
        i_understand: bool,
    },
    ImportMasterKey {
        #[arg(long)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum TelegramCmd {
    Validate {
        #[arg(long)]
        endpoint_id: Option<String>,
    },
    Dialogs {
        #[arg(long)]
        endpoint_id: Option<String>,
        #[arg(long, default_value_t = 200)]
        limit: u32,
        #[arg(long)]
        include_users: bool,
    },
    WaitChat {
        #[arg(long)]
        endpoint_id: Option<String>,
        #[arg(long, default_value_t = 60)]
        timeout_secs: u32,
        #[arg(long)]
        include_users: bool,
    },
}

#[derive(Subcommand)]
enum SnapshotsCmd {
    List {
        #[arg(long, default_value_t = 20)]
        limit: u32,
    },
}

#[derive(Subcommand)]
enum StatsCmd {
    Get,
    Last {
        #[arg(long)]
        source: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum StatusCmd {
    Get,
    Stream,
}

#[derive(Subcommand)]
enum BackupCmd {
    Run {
        #[arg(long)]
        target_id: Option<String>,
        #[arg(long)]
        source: Option<PathBuf>,
        #[arg(long, default_value = "manual")]
        label: String,
        #[arg(long)]
        no_remote_index_sync: bool,
    },
}

#[derive(Subcommand)]
enum RestoreCmd {
    Run {
        #[arg(long)]
        snapshot_id: String,
        #[arg(long)]
        target: PathBuf,
    },
    ListLatest {
        #[arg(long)]
        endpoint_id: Option<String>,
    },
    Latest {
        #[arg(long)]
        target_id: Option<String>,
        #[arg(long)]
        source_path: Option<PathBuf>,
        #[arg(long)]
        target: PathBuf,
    },
}

#[derive(Subcommand)]
enum VerifyCmd {
    Run {
        #[arg(long)]
        snapshot_id: String,
    },
    Latest {
        #[arg(long)]
        target_id: Option<String>,
        #[arg(long)]
        source_path: Option<PathBuf>,
    },
}

type Settings = settings_config::SettingsV2;

#[derive(Debug, Serialize)]
struct CliError {
    code: &'static str,
    message: String,
    details: serde_json::Value,
    retryable: bool,
}

impl CliError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: serde_json::json!({}),
            retryable: false,
        }
    }

    fn retryable(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            details: serde_json::json!({}),
            retryable: true,
        }
    }

    fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = details;
        self
    }
}

struct NdjsonProgressSink {
    task_id: String,
    throttle: Mutex<ProgressThrottle>,
}

static VAULT_KEY_CACHE: OnceLock<[u8; 32]> = OnceLock::new();

#[derive(Debug)]
struct ProgressThrottle {
    interval: Duration,
    last_emit_at: Option<Instant>,
    last_phase: Option<String>,
}

impl ProgressThrottle {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_emit_at: None,
            last_phase: None,
        }
    }

    fn should_emit(&mut self, phase: &str) -> bool {
        let now = Instant::now();

        // Always emit the first event, and whenever the phase changes (UI wants immediate
        // "phase flipped" feedback even if the progress cadence is throttled).
        if self.last_phase.as_deref() != Some(phase) {
            self.last_phase = Some(phase.to_string());
            self.last_emit_at = Some(now);
            return true;
        }

        match self.last_emit_at {
            None => {
                self.last_emit_at = Some(now);
                true
            }
            Some(last) if now.duration_since(last) >= self.interval => {
                self.last_emit_at = Some(now);
                true
            }
            _ => false,
        }
    }
}

impl ProgressSink for NdjsonProgressSink {
    fn on_progress(&self, p: televy_backup_core::TaskProgress) {
        // Throttle progress emission to avoid overwhelming the GUI (and the UI log sink)
        // when the core calls `on_progress` at very high frequency (e.g. per chunk).
        let should_emit = self
            .throttle
            .lock()
            .expect("progress throttle mutex poisoned")
            .should_emit(&p.phase);
        if !should_emit {
            return;
        }

        // Many phases don't have a stable "total" upfront. In those cases, the core currently
        // reports `*_total == *_done` as a "so far" counter which makes UI progress bars look
        // stuck at 100%. Only surface totals when they look meaningful.
        let chunks_total = match (p.phase.as_str(), p.chunks_total, p.chunks_done) {
            ("scan" | "upload" | "index" | "index_sync", Some(total), Some(done))
                if total > 0 && total == done =>
            {
                None
            }
            (_phase, other, _done) => other,
        };

        let line = serde_json::json!({
            "type": "task.progress",
            "taskId": self.task_id,
            "phase": p.phase,
            "filesTotal": p.files_total,
            "filesDone": p.files_done,
            "chunksTotal": chunks_total,
            "chunksDone": p.chunks_done,
            "bytesRead": p.bytes_read,
            "bytesUploaded": p.bytes_uploaded,
            "bytesDeduped": p.bytes_deduped,
        });
        emit_event_stdout(line);
    }
}

fn emit_event_stdout(line: serde_json::Value) {
    // In `--events` mode, stdout is typically piped to the macOS GUI. Force line delivery so the
    // UI does not get "task.state" only when the process exits (block-buffered stdout).
    println!("{line}");
    let _ = std::io::stdout().flush();
}

fn emit_task_state_running(
    events: bool,
    task_id: &str,
    kind: &str,
    target_id: Option<&str>,
    snapshot_id: Option<&str>,
) {
    if !events {
        return;
    }
    let mut obj = serde_json::Map::new();
    obj.insert(
        "type".to_string(),
        serde_json::Value::String("task.state".to_string()),
    );
    obj.insert(
        "taskId".to_string(),
        serde_json::Value::String(task_id.to_string()),
    );
    obj.insert(
        "kind".to_string(),
        serde_json::Value::String(kind.to_string()),
    );
    obj.insert(
        "state".to_string(),
        serde_json::Value::String("running".to_string()),
    );
    if let Some(t) = target_id {
        obj.insert(
            "targetId".to_string(),
            serde_json::Value::String(t.to_string()),
        );
    }
    if let Some(s) = snapshot_id {
        obj.insert(
            "snapshotId".to_string(),
            serde_json::Value::String(s.to_string()),
        );
    }
    emit_event_stdout(serde_json::Value::Object(obj));
}

fn emit_task_progress_preflight(events: bool, task_id: &str) {
    if !events {
        return;
    }
    emit_event_stdout(serde_json::json!({
        "type": "task.progress",
        "taskId": task_id,
        "phase": "preflight",
        "filesTotal": serde_json::Value::Null,
        "filesDone": serde_json::Value::Null,
        "chunksTotal": serde_json::Value::Null,
        "chunksDone": serde_json::Value::Null,
        "bytesRead": 0,
        "bytesUploaded": 0,
        "bytesDeduped": 0,
    }));
}

fn emit_task_state_failed(
    events: bool,
    task_id: &str,
    kind: &str,
    target_id: Option<&str>,
    snapshot_id: Option<&str>,
    e: &CliError,
) {
    if !events {
        return;
    }
    let mut obj = serde_json::Map::new();
    obj.insert(
        "type".to_string(),
        serde_json::Value::String("task.state".to_string()),
    );
    obj.insert(
        "taskId".to_string(),
        serde_json::Value::String(task_id.to_string()),
    );
    obj.insert(
        "kind".to_string(),
        serde_json::Value::String(kind.to_string()),
    );
    obj.insert(
        "state".to_string(),
        serde_json::Value::String("failed".to_string()),
    );
    if let Some(t) = target_id {
        obj.insert(
            "targetId".to_string(),
            serde_json::Value::String(t.to_string()),
        );
    }
    if let Some(s) = snapshot_id {
        obj.insert(
            "snapshotId".to_string(),
            serde_json::Value::String(s.to_string()),
        );
    }
    obj.insert(
        "error".to_string(),
        serde_json::json!({ "code": e.code, "message": e.message.clone() }),
    );
    emit_event_stdout(serde_json::Value::Object(obj));
}

#[derive(Clone, Copy)]
struct RunCtx<'a> {
    target_id: Option<&'a str>,
    endpoint_id: Option<&'a str>,
    source_path: Option<&'a str>,
    snapshot_id: Option<&'a str>,
}

fn emit_preflight_failed(
    events: bool,
    task_id: &str,
    kind: &str,
    run_log_path: &Path,
    started: Instant,
    ctx: RunCtx<'_>,
    e: CliError,
) -> Result<(), CliError> {
    tracing::warn!(
        event = "run.start",
        kind,
        run_id = %task_id,
        task_id = %task_id,
        target_id = ctx.target_id.unwrap_or(""),
        endpoint_id = ctx.endpoint_id.unwrap_or(""),
        source_path = ctx.source_path.unwrap_or(""),
        snapshot_id = ctx.snapshot_id.unwrap_or(""),
        log_path = %run_log_path.display(),
        "run.start"
    );

    let duration_seconds = started.elapsed().as_secs_f64();
    tracing::error!(
        event = "run.finish",
        kind,
        run_id = %task_id,
        task_id = %task_id,
        target_id = ctx.target_id.unwrap_or(""),
        endpoint_id = ctx.endpoint_id.unwrap_or(""),
        source_path = ctx.source_path.unwrap_or(""),
        snapshot_id = ctx.snapshot_id.unwrap_or(""),
        status = "failed",
        duration_seconds,
        error_code = e.code,
        error_message = %e.message,
        retryable = e.retryable,
        "run.finish"
    );

    emit_task_state_failed(events, task_id, kind, ctx.target_id, ctx.snapshot_id, &e);
    Err(e)
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let code = match run(cli).await {
        Ok(()) => 0,
        Err(e) => {
            emit_error(&e);
            1
        }
    };
    std::process::exit(code);
}

async fn run(cli: Cli) -> Result<(), CliError> {
    let config_dir = cli
        .config_dir
        .or_else(|| {
            std::env::var("TELEVYBACKUP_CONFIG_DIR")
                .ok()
                .map(PathBuf::from)
        })
        .unwrap_or_else(default_config_dir);
    let data_dir = cli
        .data_dir
        .or_else(|| {
            std::env::var("TELEVYBACKUP_DATA_DIR")
                .ok()
                .map(PathBuf::from)
        })
        .unwrap_or_else(default_data_dir);

    match cli.cmd {
        Command::Ping { value } => {
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({ "value": format!("pong: {value}") })
                );
            } else {
                println!("pong: {value}");
            }
            Ok(())
        }
        Command::Settings { cmd } => match cmd {
            SettingsCmd::Get { with_secrets } => {
                settings_get(&config_dir, &data_dir, cli.json, with_secrets).await
            }
            SettingsCmd::Set => settings_set(&config_dir, cli.json).await,
            SettingsCmd::ExportBundle { hint } => {
                settings_export_bundle(&config_dir, &data_dir, cli.json, hint).await
            }
            SettingsCmd::ImportBundle { dry_run, apply } => {
                if dry_run == apply {
                    return Err(CliError::new(
                        "config.invalid",
                        "must pass exactly one of: --dry-run, --apply",
                    ));
                }
                if dry_run {
                    settings_import_bundle_dry_run(&config_dir, &data_dir, cli.json).await
                } else {
                    settings_import_bundle_apply(&config_dir, &data_dir, cli.json).await
                }
            }
        },
        Command::Secrets { cmd } => match cmd {
            SecretsCmd::SetTelegramBotToken { endpoint_id } => {
                secrets_set_telegram_bot_token(&config_dir, &data_dir, endpoint_id, cli.json).await
            }
            SecretsCmd::SetTelegramApiHash => {
                secrets_set_telegram_api_hash(&config_dir, &data_dir, cli.json).await
            }
            SecretsCmd::ClearTelegramMtprotoSession => {
                secrets_clear_telegram_mtproto_session(&config_dir, &data_dir, cli.json).await
            }
            SecretsCmd::MigrateKeychain => {
                secrets_migrate_keychain(&config_dir, &data_dir, cli.json).await
            }
            SecretsCmd::InitMasterKey => {
                secrets_init_master_key(&config_dir, &data_dir, cli.json).await
            }
            SecretsCmd::ExportMasterKey { i_understand } => {
                secrets_export_master_key(&config_dir, &data_dir, i_understand, cli.json).await
            }
            SecretsCmd::ImportMasterKey { force } => {
                secrets_import_master_key(&config_dir, &data_dir, force, cli.json).await
            }
        },
        Command::Telegram { cmd } => match cmd {
            TelegramCmd::Validate { endpoint_id } => {
                telegram_validate(&config_dir, &data_dir, endpoint_id, cli.json).await
            }
            TelegramCmd::Dialogs {
                endpoint_id,
                limit,
                include_users,
            } => {
                telegram_dialogs(
                    &config_dir,
                    &data_dir,
                    endpoint_id,
                    limit,
                    include_users,
                    cli.json,
                )
                .await
            }
            TelegramCmd::WaitChat {
                endpoint_id,
                timeout_secs,
                include_users,
            } => {
                telegram_wait_chat(
                    &config_dir,
                    &data_dir,
                    endpoint_id,
                    timeout_secs,
                    include_users,
                    cli.json,
                )
                .await
            }
        },
        Command::Snapshots { cmd } => match cmd {
            SnapshotsCmd::List { limit } => snapshots_list(&data_dir, limit, cli.json).await,
        },
        Command::Stats { cmd } => match cmd {
            StatsCmd::Get => stats_get(&data_dir, cli.json).await,
            StatsCmd::Last { source } => stats_last(&data_dir, source, cli.json).await,
        },
        Command::Status { cmd } => match cmd {
            StatusCmd::Get => status_get(&config_dir, &data_dir, cli.json).await,
            StatusCmd::Stream => status_stream(&config_dir, &data_dir, cli.json).await,
        },
        Command::Backup { cmd } => match cmd {
            BackupCmd::Run {
                target_id,
                source,
                label,
                no_remote_index_sync,
            } => {
                backup_run(
                    &config_dir,
                    &data_dir,
                    target_id,
                    source,
                    label,
                    no_remote_index_sync,
                    cli.json,
                    cli.events,
                )
                .await
            }
        },
        Command::Restore { cmd } => match cmd {
            RestoreCmd::Run {
                snapshot_id,
                target,
            } => {
                restore_run(
                    &config_dir,
                    &data_dir,
                    snapshot_id,
                    target,
                    cli.json,
                    cli.events,
                )
                .await
            }
            RestoreCmd::ListLatest { endpoint_id } => {
                restore_list_latest(&config_dir, &data_dir, endpoint_id, cli.json).await
            }
            RestoreCmd::Latest {
                target_id,
                source_path,
                target,
            } => {
                restore_latest(
                    &config_dir,
                    &data_dir,
                    target_id,
                    source_path,
                    target,
                    cli.json,
                    cli.events,
                )
                .await
            }
        },
        Command::Verify { cmd } => match cmd {
            VerifyCmd::Run { snapshot_id } => {
                verify_run(&config_dir, &data_dir, snapshot_id, cli.json, cli.events).await
            }
            VerifyCmd::Latest {
                target_id,
                source_path,
            } => {
                verify_latest(
                    &config_dir,
                    &data_dir,
                    target_id,
                    source_path,
                    cli.json,
                    cli.events,
                )
                .await
            }
        },
    }
}

async fn status_get(config_dir: &Path, data_dir: &Path, json: bool) -> Result<(), CliError> {
    let snap = match read_status_snapshot_from_ipc(data_dir).await {
        Ok(s) => s,
        Err(e) if e.code == "status.unavailable" => {
            read_status_snapshot_from_file(config_dir, data_dir)?
        }
        Err(e) => return Err(e),
    };

    if json {
        println!(
            "{}",
            serde_json::to_string(&snap)
                .map_err(|e| CliError::new("status.invalid", e.to_string()))?
        );
    } else {
        println!(
            "status: schemaVersion={} generatedAt={} targets={}",
            snap.schema_version,
            snap.generated_at,
            snap.targets.len()
        );
    }

    Ok(())
}

async fn status_stream(config_dir: &Path, data_dir: &Path, json: bool) -> Result<(), CliError> {
    if !json {
        return Err(CliError::new(
            "cli.invalid",
            "--json is required for status stream",
        ));
    }

    #[cfg(unix)]
    {
        if let Ok(stream) = connect_status_ipc(data_dir).await {
            return status_stream_ipc(stream).await;
        }
    }

    status_stream_file(config_dir, data_dir).await
}

#[derive(Default)]
struct StatusStreamEnricher {
    started: Option<Instant>,
    totals_by_target: HashMap<String, u64>,
    smoothed_rate_by_target: HashMap<String, f64>,
    prev_sample_at: Option<Instant>,
    prev_uploaded_by_target: HashMap<String, u64>,
}

impl StatusStreamEnricher {
    fn enrich(&mut self, snap: &mut televy_backup_core::status::StatusSnapshot) {
        let started = *self.started.get_or_insert_with(Instant::now);

        let now_ms = televy_backup_core::status::now_unix_ms();
        let stale_age_ms = now_ms.saturating_sub(snap.generated_at);
        let now = Instant::now();
        let dt = self
            .prev_sample_at
            .map(|t| now.duration_since(t).as_secs_f64())
            .unwrap_or(0.0);

        let mut global_up_bps: u64 = 0;
        let mut any_target_rate = false;
        let mut global_up_total: u64 = 0;

        for t in &mut snap.targets {
            let bytes_uploaded_now = t
                .progress
                .as_ref()
                .and_then(|p| p.bytes_uploaded)
                .unwrap_or(0);

            let prev_bytes = self
                .prev_uploaded_by_target
                .get(&t.target_id)
                .copied()
                .unwrap_or(bytes_uploaded_now);

            // A new run may reset counters to 0; avoid carrying stale EWMA rates across runs.
            let reset = bytes_uploaded_now < prev_bytes;
            if reset || t.state != "running" {
                self.smoothed_rate_by_target.remove(&t.target_id);
            }

            let base = if reset {
                bytes_uploaded_now
            } else {
                prev_bytes
            };
            let delta = bytes_uploaded_now.saturating_sub(base);
            let total = self
                .totals_by_target
                .entry(t.target_id.clone())
                .and_modify(|v| *v = v.saturating_add(delta))
                .or_insert(delta);

            // Realtime rates only make sense when updates are timely.
            let mut bps: Option<u64> = None;
            if t.state == "running" && stale_age_ms <= 2_000 && dt > 0.0 {
                let raw = (delta as f64) / dt;
                let prev = self
                    .smoothed_rate_by_target
                    .get(&t.target_id)
                    .copied()
                    .unwrap_or(raw);
                // 1.0s window EWMA.
                let alpha = 1.0 - (-dt).exp();
                let smoothed = prev * (1.0 - alpha) + raw * alpha;
                self.smoothed_rate_by_target
                    .insert(t.target_id.clone(), smoothed);
                bps = Some(smoothed.max(0.0).round() as u64);
            }

            t.up.bytes_per_second = bps;
            t.up_total.bytes = Some(*total);
            if let Some(bps) = bps {
                any_target_rate = true;
                global_up_bps = global_up_bps.saturating_add(bps);
            }
            global_up_total = global_up_total.saturating_add(*total);

            self.prev_uploaded_by_target
                .insert(t.target_id.clone(), bytes_uploaded_now);
        }

        snap.global.up.bytes_per_second = if any_target_rate {
            Some(global_up_bps)
        } else {
            None
        };
        snap.global.down.bytes_per_second = None;
        snap.global.up_total.bytes = Some(global_up_total);
        snap.global.down_total.bytes = None;
        snap.global.ui_uptime_seconds = Some(started.elapsed().as_secs_f64());

        self.prev_sample_at = Some(now);
    }
}

async fn status_stream_ipc(stream: impl tokio::io::AsyncRead + Unpin) -> Result<(), CliError> {
    let mut enricher = StatusStreamEnricher::default();
    let mut lines = tokio::io::BufReader::new(stream).lines();

    let first = tokio::time::timeout(Duration::from_millis(500), lines.next_line())
        .await
        .map_err(|_| CliError::retryable("status.unavailable", "ipc status stream timed out"))?
        .map_err(|e| CliError::retryable("status.unavailable", e.to_string()))?
        .ok_or_else(|| CliError::retryable("status.unavailable", "ipc status stream ended"))?;

    let mut snap: televy_backup_core::status::StatusSnapshot =
        serde_json::from_str(&first).map_err(|e| CliError::new("status.invalid", e.to_string()))?;
    enricher.enrich(&mut snap);
    println!(
        "{}",
        serde_json::to_string(&snap).map_err(|e| CliError::new("status.invalid", e.to_string()))?
    );
    let _ = std::io::stdout().flush();

    while let Some(line) = lines
        .next_line()
        .await
        .map_err(|e| CliError::retryable("status.unavailable", e.to_string()))?
    {
        if line.trim().is_empty() {
            continue;
        }

        let mut snap: televy_backup_core::status::StatusSnapshot = serde_json::from_str(&line)
            .map_err(|e| CliError::new("status.invalid", e.to_string()))?;
        enricher.enrich(&mut snap);

        let out = serde_json::to_string(&snap)
            .map_err(|e| CliError::new("status.invalid", e.to_string()))?;
        println!("{out}");
        let _ = std::io::stdout().flush();
    }

    Err(CliError::retryable(
        "status.unavailable",
        "ipc status stream ended",
    ))
}

async fn status_stream_file(_config_dir: &Path, data_dir: &Path) -> Result<(), CliError> {
    let path = televy_backup_core::status::status_json_path(data_dir);
    let mut enricher = StatusStreamEnricher::default();

    // If status.json is missing or invalid, treat it as unavailable (no synthetic snapshots).
    let mut first = televy_backup_core::status::read_status_snapshot_json(&path).map_err(|e| {
        CliError::retryable("status.unavailable", "status source unavailable").with_details(
            serde_json::json!({
                "statusJsonPath": path.display().to_string(),
                "error": e.to_string(),
            }),
        )
    })?;

    loop {
        let is_daemon = first.source.kind == "daemon";
        let now_ms = televy_backup_core::status::now_unix_ms();
        let stale_age_ms = now_ms.saturating_sub(first.generated_at);
        let any_running = first.targets.iter().any(|t| t.state == "running");

        enricher.enrich(&mut first);

        let line = serde_json::to_string(&first)
            .map_err(|e| CliError::new("status.invalid", e.to_string()))?;
        println!("{line}");
        let _ = std::io::stdout().flush();

        let sleep = if is_daemon && any_running && stale_age_ms <= 2_000 {
            std::time::Duration::from_millis(100)
        } else {
            std::time::Duration::from_secs(1)
        };
        tokio::time::sleep(sleep).await;

        first = televy_backup_core::status::read_status_snapshot_json(&path).map_err(|e| {
            CliError::retryable("status.unavailable", "status source unavailable").with_details(
                serde_json::json!({
                    "statusJsonPath": path.display().to_string(),
                    "error": e.to_string(),
                }),
            )
        })?;
    }
}

#[cfg(unix)]
async fn connect_status_ipc(data_dir: &Path) -> std::io::Result<UnixStream> {
    let socket_path = televy_backup_core::status::status_ipc_socket_path(data_dir);
    UnixStream::connect(socket_path).await
}

#[cfg(not(unix))]
async fn connect_status_ipc(_data_dir: &Path) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "status IPC is only supported on unix",
    ))
}

#[cfg(not(unix))]
async fn read_status_snapshot_from_ipc(
    _data_dir: &Path,
) -> Result<televy_backup_core::status::StatusSnapshot, CliError> {
    Err(CliError::retryable(
        "status.unavailable",
        "status IPC is only supported on unix",
    ))
}

#[cfg(unix)]
async fn read_status_snapshot_from_ipc(
    data_dir: &Path,
) -> Result<televy_backup_core::status::StatusSnapshot, CliError> {
    let stream = connect_status_ipc(data_dir).await.map_err(|e| {
        CliError::retryable("status.unavailable", "status ipc unavailable").with_details(
            serde_json::json!({
                "socketPath": televy_backup_core::status::status_ipc_socket_path(data_dir).display().to_string(),
                "error": e.to_string(),
            }),
        )
    })?;

    let mut lines = tokio::io::BufReader::new(stream).lines();
    let line = tokio::time::timeout(Duration::from_millis(500), lines.next_line())
        .await
        .map_err(|_| CliError::retryable("status.unavailable", "ipc status get timed out"))?
        .map_err(|e| CliError::retryable("status.unavailable", e.to_string()))?
        .ok_or_else(|| CliError::retryable("status.unavailable", "ipc status get ended"))?;

    serde_json::from_str(&line).map_err(|e| CliError::new("status.invalid", e.to_string()))
}

fn read_status_snapshot_from_file(
    config_dir: &Path,
    data_dir: &Path,
) -> Result<televy_backup_core::status::StatusSnapshot, CliError> {
    let _ = config_dir; // reserved for future compatibility checks / synthetic snapshots
    let path = televy_backup_core::status::status_json_path(data_dir);
    televy_backup_core::status::read_status_snapshot_json(&path).map_err(|e| {
        CliError::retryable("status.unavailable", "status source unavailable").with_details(
            serde_json::json!({
                "statusJsonPath": path.display().to_string(),
                "error": e.to_string(),
            }),
        )
    })
}

async fn settings_get(
    config_dir: &Path,
    data_dir: &Path,
    json: bool,
    with_secrets: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;

    if json {
        if with_secrets {
            // The macOS UI calls `settings get --with-secrets` unconditionally.
            // Keep settings readable even when control IPC isn't available.
            match daemon_control_secrets_presence(data_dir, None) {
                Ok(secrets) => {
                    println!(
                        "{}",
                        serde_json::json!({ "settings": settings, "secrets": secrets })
                    );
                }
                Err(e) => {
                    println!(
                        "{}",
                        serde_json::json!({
                            "settings": settings,
                            "secrets": serde_json::Value::Null,
                            "secretsError": { "code": e.code, "message": e.message, "retryable": e.retryable }
                        })
                    );
                }
            }
        } else {
            println!("{}", serde_json::json!({ "settings": settings }));
        }
    } else {
        let text = toml::to_string(&settings)
            .map_err(|e| CliError::new("config.invalid", e.to_string()))?;
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
        if with_secrets {
            match daemon_control_secrets_presence(data_dir, None) {
                Ok(secrets) => {
                    let master_present = secrets
                        .get("masterKeyPresent")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    let mtproto_api_hash_present = secrets
                        .get("telegramMtprotoApiHashPresent")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    println!();
                    println!("masterKeyPresent={master_present}");
                    println!("telegramMtprotoApiHashPresent={mtproto_api_hash_present}");
                    for ep in &settings.telegram_endpoints {
                        let telegram_present = secrets
                            .get("telegramBotTokenPresentByEndpoint")
                            .and_then(|m| m.get(&ep.id))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let mtproto_session_present = secrets
                            .get("telegramMtprotoSessionPresentByEndpoint")
                            .and_then(|m| m.get(&ep.id))
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        println!(
                            "telegramBotTokenPresent[{id}]={telegram_present}",
                            id = ep.id
                        );
                        println!(
                            "telegramMtprotoSessionPresent[{id}]={mtproto_session_present}",
                            id = ep.id
                        );
                    }
                }
                Err(e) => {
                    println!();
                    println!("secretsErrorCode={}", e.code);
                    println!("secretsErrorMessage={}", e.message);
                    println!("secretsErrorRetryable={}", e.retryable);
                }
            }
        }
    }
    Ok(())
}

async fn settings_set(config_dir: &Path, json: bool) -> Result<(), CliError> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let settings: Settings = settings_config::parse_settings_v2(&input)
        .map_err(|e| CliError::new("config.invalid", e.to_string()))?;
    settings_config::save_settings_v2(config_dir, &settings).map_err(map_core_err)?;

    if json {
        println!("{}", serde_json::json!({ "settings": settings }));
    }
    Ok(())
}

#[derive(Debug, serde::Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum LocalMasterKeyState {
    Missing,
    Match,
    Mismatch,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum ConfigBundleNextAction {
    Apply,
    StartKeyRotation,
}

#[derive(Debug, serde::Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ConfigBundleBootstrapState {
    Ok,
    Missing,
    Invalid,
}

#[derive(Debug, serde::Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ConfigBundleRemoteLatestState {
    Ok,
    Missing,
}

#[derive(Debug, serde::Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ConfigBundleLocalIndexState {
    Match,
    Stale,
    Missing,
}

#[derive(Debug, serde::Serialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ConfigBundleConflictState {
    None,
    NeedsResolution,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsExportBundleJson {
    bundle_key: String,
    format: String,
}

const CONFIG_BUNDLE_PASSPHRASE_ENV: &str = "TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE";

fn load_config_bundle_passphrase() -> Result<String, CliError> {
    let passphrase = std::env::var(CONFIG_BUNDLE_PASSPHRASE_ENV).unwrap_or_default();
    if passphrase.trim().is_empty() {
        return Err(CliError::new(
            "config_bundle.passphrase_required",
            format!("missing {CONFIG_BUNDLE_PASSPHRASE_ENV}"),
        ));
    }
    Ok(passphrase)
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunJson {
    format: String,
    local_master_key: LocalMasterKeyJson,
    local_has_targets: bool,
    next_action: ConfigBundleNextAction,
    bundle: SettingsImportBundleDryRunBundleJson,
    preflight: SettingsImportBundleDryRunPreflightJson,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct LocalMasterKeyJson {
    state: LocalMasterKeyState,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunBundleJson {
    settings_version: u32,
    targets: Vec<SettingsImportBundleDryRunTargetJson>,
    endpoints: Vec<SettingsImportBundleDryRunEndpointJson>,
    secrets_coverage: SettingsImportBundleDryRunSecretsCoverageJson,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunTargetJson {
    id: String,
    source_path: String,
    endpoint_id: String,
    label: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunEndpointJson {
    id: String,
    chat_id: String,
    mode: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunSecretsCoverageJson {
    present_keys: Vec<String>,
    excluded_keys: Vec<String>,
    missing_keys: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunPreflightJson {
    targets: Vec<SettingsImportBundleDryRunPreflightTargetJson>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunPreflightTargetJson {
    target_id: String,
    source_path_exists: bool,
    bootstrap: SettingsImportBundleDryRunBootstrapJson,
    remote_latest: SettingsImportBundleDryRunRemoteLatestJson,
    local_index: SettingsImportBundleDryRunLocalIndexJson,
    conflict: SettingsImportBundleDryRunConflictJson,
}

#[derive(Debug, serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunBootstrapJson {
    state: ConfigBundleBootstrapState,
    details: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunRemoteLatestJson {
    state: ConfigBundleRemoteLatestState,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_object_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunLocalIndexJson {
    state: ConfigBundleLocalIndexState,
    details: serde_json::Value,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleDryRunConflictJson {
    state: ConfigBundleConflictState,
    reasons: Vec<String>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyRequest {
    bundle_key: String,
    selected_target_ids: Vec<String>,
    confirm: SettingsImportBundleApplyConfirm,
    #[serde(default)]
    resolutions: HashMap<String, SettingsImportBundleApplyResolution>,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyConfirm {
    // Backward-compatible field: older UIs may send it, but the only user-facing confirmation
    // we require is the phrase.
    #[allow(dead_code)]
    #[serde(default)]
    ack_risks: bool,
    phrase: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "snake_case", tag = "mode")]
enum SettingsImportBundleApplyResolution {
    OverwriteLocal,
    OverwriteRemote,
    Rebind { new_source_path: String },
    Skip,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyResponse {
    ok: bool,
    local_index: SettingsImportBundleApplyLocalIndexJson,
    applied: SettingsImportBundleApplyAppliedJson,
    actions: SettingsImportBundleApplyActionsJson,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyLocalIndexJson {
    #[serde(skip_serializing_if = "Option::is_none")]
    previous_db_backup_path: Option<String>,
    rebuilt_db_path: String,
    rebuilt_from: SettingsImportBundleApplyRebuiltFromJson,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyRebuiltFromJson {
    mode: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    snapshot_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    manifest_object_id: Option<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyAppliedJson {
    targets: Vec<String>,
    endpoints: Vec<String>,
    secrets_written: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyActionsJson {
    updated_pinned_catalog: Vec<SettingsImportBundleApplyPinnedUpdateJson>,
    local_index_synced: Vec<SettingsImportBundleApplyLocalIndexSyncedJson>,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyPinnedUpdateJson {
    endpoint_id: String,
    old: String,
    new: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsImportBundleApplyLocalIndexSyncedJson {
    target_id: String,
    from: String,
    to: String,
}

async fn settings_export_bundle(
    config_dir: &Path,
    data_dir: &Path,
    json: bool,
    hint: Option<String>,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let master_key = load_master_key(config_dir, data_dir)?;
    let passphrase = load_config_bundle_passphrase()?;

    let vault_key = load_or_create_vault_key(data_dir)?;
    let secrets_path = televy_backup_core::secrets::secrets_path(config_dir);
    let store = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
        .map_err(map_secrets_store_err)?;

    // Exclude MTProto sessions: session_key is kept in settings, but the session value is
    // per-device and must not be exported.
    let mut bundle_secrets = config_bundle::ConfigBundleSecretsV2 {
        excluded: settings
            .telegram_endpoints
            .iter()
            .map(|ep| ep.mtproto.session_key.clone())
            .collect::<Vec<_>>(),
        ..Default::default()
    };

    let mut required = Vec::new();
    required.push(settings.telegram.mtproto.api_hash_key.clone());
    for ep in &settings.telegram_endpoints {
        required.push(ep.bot_token_key.clone());
    }
    required.sort();
    required.dedup();

    for key in required {
        if let Some(value) = store.get(&key) {
            bundle_secrets.entries.insert(key, value.to_string());
        } else {
            bundle_secrets.missing.push(key);
        }
    }
    bundle_secrets.excluded.sort();
    bundle_secrets.excluded.dedup();
    bundle_secrets.missing.sort();
    bundle_secrets.missing.dedup();

    let bundle_key = config_bundle::encode_config_bundle_key_v2(
        &master_key,
        &settings,
        bundle_secrets,
        &passphrase,
        hint.as_deref().unwrap_or(""),
    )
    .map_err(map_core_err)?;

    if json {
        println!(
            "{}",
            serde_json::to_string(&SettingsExportBundleJson {
                bundle_key: bundle_key.clone(),
                format: config_bundle::CONFIG_BUNDLE_FORMAT_V2.to_string(),
            })
            .map_err(|e| CliError::new("config.invalid", e.to_string()))?
        );
    } else {
        println!("{bundle_key}");
    }
    Ok(())
}

fn read_stdin_one_line() -> Result<String, CliError> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let line = input
        .lines()
        .find(|l| !l.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.is_empty() {
        return Err(CliError::new("config.invalid", "stdin is empty"));
    }
    Ok(line.to_string())
}

fn load_optional_master_key(
    config_dir: &Path,
    data_dir: &Path,
) -> Result<Option<[u8; 32]>, CliError> {
    let Some(b64) = get_secret(config_dir, data_dir, MASTER_KEY_KEY)? else {
        return Ok(None);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| CliError::new("config.invalid", e.to_string()))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|_| CliError::new("config.invalid", "invalid master key length"))?;
    Ok(Some(arr))
}

fn endpoint_index_db_path(data_dir: &Path, endpoint_id: &str) -> PathBuf {
    data_dir
        .join("index")
        .join(format!("index.{endpoint_id}.sqlite"))
}

fn legacy_global_index_db_path(data_dir: &Path) -> PathBuf {
    data_dir.join("index").join("index.sqlite")
}

async fn init_empty_index_db(path: &Path) -> Result<(), CliError> {
    // When creating a brand-new per-endpoint index DB, ensure the parent directory exists.
    // (SQLite can create the DB file, but not missing directories.)
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
    }
    let _ = televy_backup_core::index_db::open_index_db(path)
        .await
        .map_err(map_core_err)?;
    Ok(())
}

async fn settings_import_bundle_dry_run(
    config_dir: &Path,
    data_dir: &Path,
    json: bool,
) -> Result<(), CliError> {
    if !json {
        return Err(CliError::new(
            "config.invalid",
            "import-bundle requires --json",
        ));
    }

    let bundle_key = read_stdin_one_line()?;
    let passphrase = load_config_bundle_passphrase()?;
    let decoded = config_bundle::decode_config_bundle_key_v2(&bundle_key, &passphrase)
        .map_err(map_core_err)?;
    let bundle_settings = decoded.payload.settings;
    let bundle_secrets = decoded.payload.secrets;
    let bundle_master_key = decoded.master_key;

    let local_settings = load_settings(config_dir)?;
    let local_has_targets = !local_settings.targets.is_empty();
    let local_master_key = load_optional_master_key(config_dir, data_dir)?;
    let local_master_key_state = match local_master_key {
        None => LocalMasterKeyState::Missing,
        Some(k) if k == bundle_master_key => LocalMasterKeyState::Match,
        Some(_) => LocalMasterKeyState::Mismatch,
    };

    let next_action = match (local_master_key_state, local_has_targets) {
        (LocalMasterKeyState::Mismatch, true) => ConfigBundleNextAction::StartKeyRotation,
        _ => ConfigBundleNextAction::Apply,
    };

    let bundle_targets = bundle_settings
        .targets
        .iter()
        .map(|t| SettingsImportBundleDryRunTargetJson {
            id: t.id.clone(),
            source_path: t.source_path.clone(),
            endpoint_id: t.endpoint_id.clone(),
            label: t.label.clone(),
        })
        .collect::<Vec<_>>();

    let bundle_endpoints = bundle_settings
        .telegram_endpoints
        .iter()
        .map(|ep| SettingsImportBundleDryRunEndpointJson {
            id: ep.id.clone(),
            chat_id: ep.chat_id.clone(),
            mode: ep.mode.clone(),
        })
        .collect::<Vec<_>>();

    let mut present_keys = bundle_secrets.entries.keys().cloned().collect::<Vec<_>>();
    present_keys.sort();

    let mut excluded_keys = bundle_secrets.excluded.clone();
    excluded_keys.sort();

    let mut missing_keys = bundle_secrets.missing.clone();
    missing_keys.sort();

    let secrets_coverage = SettingsImportBundleDryRunSecretsCoverageJson {
        present_keys,
        excluded_keys,
        missing_keys,
    };

    let api_id = bundle_settings.telegram.mtproto.api_id;
    let api_hash = bundle_secrets
        .entries
        .get(&bundle_settings.telegram.mtproto.api_hash_key)
        .cloned();

    let mut endpoint_catalogs: HashMap<String, Option<bootstrap::BootstrapCatalogV1>> =
        HashMap::new();
    let mut endpoint_bootstrap: HashMap<String, SettingsImportBundleDryRunBootstrapJson> =
        HashMap::new();

    for ep in &bundle_settings.telegram_endpoints {
        let bot_token = bundle_secrets.entries.get(&ep.bot_token_key).cloned();
        let provider = settings_config::endpoint_provider(&ep.id);

        let Some(api_hash) = api_hash.clone() else {
            endpoint_catalogs.insert(ep.id.clone(), None);
            endpoint_bootstrap.insert(
                ep.id.clone(),
                SettingsImportBundleDryRunBootstrapJson {
                    state: ConfigBundleBootstrapState::Missing,
                    details: serde_json::json!({ "reason": "missing_api_hash" }),
                },
            );
            continue;
        };

        let Some(bot_token) = bot_token else {
            endpoint_catalogs.insert(ep.id.clone(), None);
            endpoint_bootstrap.insert(
                ep.id.clone(),
                SettingsImportBundleDryRunBootstrapJson {
                    state: ConfigBundleBootstrapState::Missing,
                    details: serde_json::json!({ "reason": "missing_bot_token" }),
                },
            );
            continue;
        };

        if api_id <= 0 {
            endpoint_catalogs.insert(ep.id.clone(), None);
            endpoint_bootstrap.insert(
                ep.id.clone(),
                SettingsImportBundleDryRunBootstrapJson {
                    state: ConfigBundleBootstrapState::Missing,
                    details: serde_json::json!({ "reason": "invalid_api_id" }),
                },
            );
            continue;
        }

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            provider: provider.clone(),
            api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: ep.chat_id.clone(),
            session: None,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(|e| map_mtproto_validate_err(e, &bot_token, &api_hash))?;

        let catalog = match bootstrap::load_remote_catalog(&storage, &bundle_master_key).await {
            Ok(Some(cat)) => {
                endpoint_bootstrap.insert(
                    ep.id.clone(),
                    SettingsImportBundleDryRunBootstrapJson {
                        state: ConfigBundleBootstrapState::Ok,
                        details: serde_json::json!({}),
                    },
                );
                Some(cat)
            }
            Ok(None) => {
                endpoint_bootstrap.insert(
                    ep.id.clone(),
                    SettingsImportBundleDryRunBootstrapJson {
                        state: ConfigBundleBootstrapState::Missing,
                        details: serde_json::json!({}),
                    },
                );
                None
            }
            Err(televy_backup_core::Error::BootstrapDecryptFailed { message }) => {
                endpoint_bootstrap.insert(
                    ep.id.clone(),
                    SettingsImportBundleDryRunBootstrapJson {
                        state: ConfigBundleBootstrapState::Invalid,
                        details: serde_json::json!({ "error": message }),
                    },
                );
                None
            }
            Err(e) => return Err(map_core_err(e)),
        };

        endpoint_catalogs.insert(ep.id.clone(), catalog);
    }

    let mut preflight_targets = Vec::new();
    for t in &bundle_settings.targets {
        let source_path_exists = Path::new(&t.source_path).exists();
        let ep_id = &t.endpoint_id;
        let provider = settings_config::endpoint_provider(ep_id);

        let bootstrap = endpoint_bootstrap.get(ep_id).cloned().unwrap_or(
            SettingsImportBundleDryRunBootstrapJson {
                state: ConfigBundleBootstrapState::Missing,
                details: serde_json::json!({ "reason": "unknown_endpoint" }),
            },
        );

        let remote_latest = match endpoint_catalogs.get(ep_id).and_then(|c| c.as_ref()) {
            Some(cat) => {
                let mut latest = cat
                    .targets
                    .iter()
                    .find(|x| x.target_id == t.id)
                    .and_then(|x| x.latest.clone());

                if latest.is_none() {
                    let matches = cat
                        .targets
                        .iter()
                        .filter(|x| x.source_path == t.source_path)
                        .collect::<Vec<_>>();
                    if matches.len() == 1 {
                        latest = matches[0].latest.clone();
                    }
                }

                match latest {
                    Some(l) => SettingsImportBundleDryRunRemoteLatestJson {
                        state: ConfigBundleRemoteLatestState::Ok,
                        snapshot_id: Some(l.snapshot_id),
                        manifest_object_id: Some(l.manifest_object_id),
                    },
                    None => SettingsImportBundleDryRunRemoteLatestJson {
                        state: ConfigBundleRemoteLatestState::Missing,
                        snapshot_id: None,
                        manifest_object_id: None,
                    },
                }
            }
            None => SettingsImportBundleDryRunRemoteLatestJson {
                state: ConfigBundleRemoteLatestState::Missing,
                snapshot_id: None,
                manifest_object_id: None,
            },
        };

        let local_db_path = endpoint_index_db_path(data_dir, ep_id);
        let mut conflict_reasons = Vec::new();
        if !source_path_exists {
            conflict_reasons.push("missing_path".to_string());
        }
        if matches!(bootstrap.state, ConfigBundleBootstrapState::Invalid) {
            conflict_reasons.push("bootstrap_invalid".to_string());
        }

        let local_index_state = if remote_latest.state == ConfigBundleRemoteLatestState::Ok {
            let snapshot_id = remote_latest.snapshot_id.as_deref().unwrap_or("");
            let manifest_object_id = remote_latest.manifest_object_id.as_deref().unwrap_or("");
            if !local_db_path.exists() {
                ConfigBundleLocalIndexState::Missing
            } else {
                let match_ok = televy_backup_core::index_sync::local_index_matches_remote_latest(
                    &local_db_path,
                    &provider,
                    snapshot_id,
                    manifest_object_id,
                )
                .await
                .map_err(map_core_err)?;
                if match_ok {
                    ConfigBundleLocalIndexState::Match
                } else {
                    ConfigBundleLocalIndexState::Stale
                }
            }
        } else {
            ConfigBundleLocalIndexState::Missing
        };

        if remote_latest.state == ConfigBundleRemoteLatestState::Ok
            && local_index_state == ConfigBundleLocalIndexState::Stale
        {
            conflict_reasons.push("local_vs_remote_mismatch".to_string());
        }

        let conflict_state = if conflict_reasons.is_empty() {
            ConfigBundleConflictState::None
        } else {
            ConfigBundleConflictState::NeedsResolution
        };

        preflight_targets.push(SettingsImportBundleDryRunPreflightTargetJson {
            target_id: t.id.clone(),
            source_path_exists,
            bootstrap,
            remote_latest,
            local_index: SettingsImportBundleDryRunLocalIndexJson {
                state: local_index_state,
                details: serde_json::json!({ "dbPath": local_db_path.display().to_string() }),
            },
            conflict: SettingsImportBundleDryRunConflictJson {
                state: conflict_state,
                reasons: conflict_reasons,
            },
        });
    }

    let out = SettingsImportBundleDryRunJson {
        format: config_bundle::CONFIG_BUNDLE_FORMAT_V2.to_string(),
        local_master_key: LocalMasterKeyJson {
            state: local_master_key_state,
        },
        local_has_targets,
        next_action,
        bundle: SettingsImportBundleDryRunBundleJson {
            settings_version: bundle_settings.version,
            targets: bundle_targets,
            endpoints: bundle_endpoints,
            secrets_coverage,
        },
        preflight: SettingsImportBundleDryRunPreflightJson {
            targets: preflight_targets,
        },
    };

    println!(
        "{}",
        serde_json::to_string(&out).map_err(|e| CliError::new("config.invalid", e.to_string()))?
    );
    Ok(())
}

async fn settings_import_bundle_apply(
    config_dir: &Path,
    data_dir: &Path,
    json: bool,
) -> Result<(), CliError> {
    if !json {
        return Err(CliError::new(
            "config.invalid",
            "import-bundle --apply requires --json",
        ));
    }

    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let req: SettingsImportBundleApplyRequest =
        serde_json::from_str(&input).map_err(|e| CliError::new("config.invalid", e.to_string()))?;

    if req.selected_target_ids.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "selectedTargetIds must not be empty",
        ));
    }
    if req.confirm.phrase != "IMPORT" {
        return Err(CliError::new(
            "config_bundle.confirm_required",
            "apply requires confirm.phrase=\"IMPORT\"",
        ));
    }

    let passphrase = load_config_bundle_passphrase()?;
    let decoded = config_bundle::decode_config_bundle_key_v2(&req.bundle_key, &passphrase)
        .map_err(map_core_err)?;
    let bundle_settings = decoded.payload.settings;
    let bundle_secrets = decoded.payload.secrets;
    let bundle_master_key = decoded.master_key;

    let local_settings = load_settings(config_dir)?;
    let local_has_targets = !local_settings.targets.is_empty();
    let local_master_key = load_optional_master_key(config_dir, data_dir)?;
    let local_master_key_state = match local_master_key {
        None => LocalMasterKeyState::Missing,
        Some(k) if k == bundle_master_key => LocalMasterKeyState::Match,
        Some(_) => LocalMasterKeyState::Mismatch,
    };
    if matches!(local_master_key_state, LocalMasterKeyState::Mismatch) && local_has_targets {
        return Err(CliError::new(
            "config_bundle.rotation_required",
            "local master key mismatch and local targets exist; start master key rotation flow",
        ));
    }

    let selected_ids: std::collections::HashSet<String> =
        req.selected_target_ids.iter().cloned().collect();

    let mut selected_targets = bundle_settings
        .targets
        .iter()
        .filter(|t| selected_ids.contains(&t.id))
        .cloned()
        .collect::<Vec<_>>();
    if selected_targets.len() != selected_ids.len() {
        return Err(CliError::new(
            "config.invalid",
            "selectedTargetIds contains unknown ids",
        ));
    }

    // Preflight only on the selected targets.
    let api_id = bundle_settings.telegram.mtproto.api_id;
    let api_hash = bundle_secrets
        .entries
        .get(&bundle_settings.telegram.mtproto.api_hash_key)
        .cloned();

    let mut endpoint_storage: HashMap<String, TelegramMtProtoStorage> = HashMap::new();
    let mut endpoint_catalogs: HashMap<String, Option<bootstrap::BootstrapCatalogV1>> =
        HashMap::new();
    let mut endpoint_bootstrap_state: HashMap<String, ConfigBundleBootstrapState> = HashMap::new();

    let mut endpoints_needed = std::collections::BTreeSet::<String>::new();
    for t in &selected_targets {
        endpoints_needed.insert(t.endpoint_id.clone());
    }

    for ep_id in endpoints_needed {
        let Some(ep) = bundle_settings
            .telegram_endpoints
            .iter()
            .find(|e| e.id == ep_id)
            .cloned()
        else {
            return Err(CliError::new(
                "config.invalid",
                format!("missing endpoint in bundle settings: {ep_id}"),
            ));
        };

        let Some(api_hash) = api_hash.clone() else {
            endpoint_catalogs.insert(ep.id.clone(), None);
            endpoint_bootstrap_state.insert(ep.id.clone(), ConfigBundleBootstrapState::Missing);
            continue;
        };
        let Some(bot_token) = bundle_secrets.entries.get(&ep.bot_token_key).cloned() else {
            endpoint_catalogs.insert(ep.id.clone(), None);
            endpoint_bootstrap_state.insert(ep.id.clone(), ConfigBundleBootstrapState::Missing);
            continue;
        };
        if api_id <= 0 {
            endpoint_catalogs.insert(ep.id.clone(), None);
            endpoint_bootstrap_state.insert(ep.id.clone(), ConfigBundleBootstrapState::Missing);
            continue;
        }

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let provider = settings_config::endpoint_provider(&ep.id);
        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            provider,
            api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: ep.chat_id.clone(),
            session: None,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(|e| map_mtproto_validate_err(e, &bot_token, &api_hash))?;

        let cat = match bootstrap::load_remote_catalog(&storage, &bundle_master_key).await {
            Ok(Some(cat)) => {
                endpoint_bootstrap_state.insert(ep.id.clone(), ConfigBundleBootstrapState::Ok);
                Some(cat)
            }
            Ok(None) => {
                endpoint_bootstrap_state.insert(ep.id.clone(), ConfigBundleBootstrapState::Missing);
                None
            }
            Err(televy_backup_core::Error::BootstrapDecryptFailed { .. }) => {
                endpoint_bootstrap_state.insert(ep.id.clone(), ConfigBundleBootstrapState::Invalid);
                None
            }
            Err(e) => return Err(map_core_err(e)),
        };

        endpoint_storage.insert(ep.id.clone(), storage);
        endpoint_catalogs.insert(ep.id.clone(), cat);
    }

    // Detect conflicts (missing_path / bootstrap_invalid / local_vs_remote_mismatch) and enforce
    // that apply provides explicit resolutions for any target needing resolution.
    for t in &selected_targets {
        let mut reasons = Vec::new();

        if !Path::new(&t.source_path).exists() {
            reasons.push("missing_path");
        }

        let bootstrap_state = endpoint_bootstrap_state
            .get(&t.endpoint_id)
            .copied()
            .unwrap_or(ConfigBundleBootstrapState::Missing);
        if bootstrap_state == ConfigBundleBootstrapState::Invalid {
            reasons.push("bootstrap_invalid");
        }

        let mut remote_latest: Option<bootstrap::BootstrapLatest> = None;
        if let Some(cat) = endpoint_catalogs
            .get(&t.endpoint_id)
            .and_then(|c| c.as_ref())
        {
            remote_latest = cat
                .targets
                .iter()
                .find(|x| x.target_id == t.id)
                .and_then(|x| x.latest.clone());

            if remote_latest.is_none() {
                let matches = cat
                    .targets
                    .iter()
                    .filter(|x| x.source_path == t.source_path)
                    .collect::<Vec<_>>();
                if matches.len() == 1 {
                    remote_latest = matches[0].latest.clone();
                }
            }
        }

        if let Some(latest) = &remote_latest {
            let provider = settings_config::endpoint_provider(&t.endpoint_id);
            let db_path = endpoint_index_db_path(data_dir, &t.endpoint_id);
            if db_path.exists() {
                let match_ok = televy_backup_core::index_sync::local_index_matches_remote_latest(
                    &db_path,
                    &provider,
                    &latest.snapshot_id,
                    &latest.manifest_object_id,
                )
                .await
                .map_err(map_core_err)?;
                if !match_ok {
                    reasons.push("local_vs_remote_mismatch");
                }
            }
        }

        if reasons.is_empty() {
            continue;
        }

        let res = req.resolutions.get(&t.id).ok_or_else(|| {
            CliError::new(
                "config_bundle.conflict",
                format!(
                    "missing resolution for target {} ({})",
                    t.id,
                    reasons.join(",")
                ),
            )
        })?;

        if reasons.contains(&"missing_path") {
            match res {
                SettingsImportBundleApplyResolution::Rebind { .. }
                | SettingsImportBundleApplyResolution::Skip => {}
                _ => {
                    return Err(CliError::new(
                        "config_bundle.conflict",
                        format!("target {} missing_path; must choose rebind or skip", t.id),
                    ));
                }
            }
        }

        if reasons.contains(&"bootstrap_invalid")
            && !matches!(res, SettingsImportBundleApplyResolution::Skip)
        {
            return Err(CliError::new(
                "config_bundle.conflict",
                format!("target {} bootstrap invalid; must skip", t.id),
            ));
        }
    }

    let mut updated_pins = Vec::new();

    // Apply resolutions that require remote writes first (overwrite_remote), so index rebuild can
    // download the updated latest if needed.
    for t in &selected_targets {
        let Some(resolution) = req.resolutions.get(&t.id) else {
            continue;
        };
        if !matches!(
            resolution,
            SettingsImportBundleApplyResolution::OverwriteRemote
        ) {
            continue;
        }

        let bootstrap_state = endpoint_bootstrap_state
            .get(&t.endpoint_id)
            .copied()
            .unwrap_or(ConfigBundleBootstrapState::Missing);
        if bootstrap_state == ConfigBundleBootstrapState::Invalid {
            return Err(CliError::new(
                "config_bundle.conflict",
                format!("target {} bootstrap invalid; must skip", t.id),
            ));
        }

        let Some(storage) = endpoint_storage.get(&t.endpoint_id) else {
            return Err(CliError::new(
                "config_bundle.conflict",
                format!(
                    "target {} overwrite_remote requires telegram access (missing secrets?)",
                    t.id
                ),
            ));
        };

        // Determine local head from the existing per-endpoint DB.
        let provider = settings_config::endpoint_provider(&t.endpoint_id);
        let db_path = endpoint_index_db_path(data_dir, &t.endpoint_id);
        if !db_path.exists() {
            return Err(CliError::new(
                "config_bundle.conflict",
                format!(
                    "target {} overwrite_remote requires existing local index DB: {}",
                    t.id,
                    db_path.display()
                ),
            ));
        }

        let pool = televy_backup_core::index_db::open_existing_index_db(&db_path)
            .await
            .map_err(map_core_err)?;

        let row: Option<sqlx::sqlite::SqliteRow> = sqlx::query(
            r#"
            SELECT s.snapshot_id AS snapshot_id, r.manifest_object_id AS manifest_object_id
            FROM snapshots s
            JOIN remote_indexes r ON r.snapshot_id = s.snapshot_id
            WHERE s.source_path = ? AND (r.provider = ? OR r.provider LIKE ?)
            ORDER BY s.created_at DESC
            LIMIT 1
            "#,
        )
        .bind(&t.source_path)
        .bind(&provider)
        .bind({
            let kind = provider
                .split(['/', ':'])
                .next()
                .unwrap_or(provider.as_str());
            format!("{kind}%")
        })
        .fetch_optional(&pool)
        .await
        .map_err(|e| CliError::new("db.failed", e.to_string()))?;

        let Some(row) = row else {
            return Err(CliError::new(
                "config_bundle.conflict",
                format!("target {} has no local snapshot to pin", t.id),
            ));
        };

        let snapshot_id: String = row.get("snapshot_id");
        let manifest_object_id: String = row.get("manifest_object_id");

        let old = storage.get_pinned_object_id().map_err(map_core_err)?;
        bootstrap::update_remote_latest(
            storage,
            &bundle_master_key,
            &t.id,
            &t.source_path,
            &t.label,
            &snapshot_id,
            &manifest_object_id,
        )
        .await
        .map_err(map_core_err)?;
        let new = storage
            .get_pinned_object_id()
            .map_err(map_core_err)?
            .unwrap_or_else(|| "—".to_string());

        updated_pins.push(SettingsImportBundleApplyPinnedUpdateJson {
            endpoint_id: t.endpoint_id.clone(),
            old: old.unwrap_or_else(|| "—".to_string()),
            new,
        });
    }

    // Refresh catalogs for endpoints where we updated pins.
    if !updated_pins.is_empty() {
        for ep_id in updated_pins.iter().map(|p| p.endpoint_id.clone()) {
            if let Some(storage) = endpoint_storage.get(&ep_id) {
                let cat = match bootstrap::load_remote_catalog(storage, &bundle_master_key).await {
                    Ok(Some(cat)) => Some(cat),
                    Ok(None) => None,
                    Err(televy_backup_core::Error::BootstrapDecryptFailed { .. }) => None,
                    Err(e) => return Err(map_core_err(e)),
                };
                endpoint_catalogs.insert(ep_id, cat);
            }
        }
    }

    // Filter targets by resolutions (skip/rebind) and validate constraints.
    selected_targets = selected_targets
        .into_iter()
        .filter_map(|mut t| {
            let res = req.resolutions.get(&t.id);
            match res {
                Some(SettingsImportBundleApplyResolution::Skip) => None,
                Some(SettingsImportBundleApplyResolution::Rebind { new_source_path }) => {
                    t.source_path = new_source_path.clone();
                    Some(t)
                }
                _ => Some(t),
            }
        })
        .collect();

    if selected_targets.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "all selected targets were skipped",
        ));
    }

    for t in &selected_targets {
        if !Path::new(&t.source_path).exists() {
            return Err(CliError::new(
                "config_bundle.conflict",
                format!(
                    "target {} source_path does not exist after resolution; choose rebind to an existing path or skip",
                    t.id
                ),
            ));
        }

        let bootstrap_state = endpoint_bootstrap_state
            .get(&t.endpoint_id)
            .copied()
            .unwrap_or(ConfigBundleBootstrapState::Missing);
        if bootstrap_state == ConfigBundleBootstrapState::Invalid {
            return Err(CliError::new(
                "config_bundle.conflict",
                format!("target {} bootstrap invalid; must skip", t.id),
            ));
        }
    }

    // Index rebuild: per endpoint, backup existing DB then rebuild from remote latest (or empty).
    let mut rebuilt_db_path = String::new();
    let mut previous_backup_path: Option<String> = None;
    let mut rebuilt_from = SettingsImportBundleApplyRebuiltFromJson {
        mode: "empty".to_string(),
        snapshot_id: None,
        manifest_object_id: None,
    };
    let mut local_index_synced = Vec::new();

    let mut endpoints_to_rebuild = std::collections::BTreeSet::<String>::new();
    for t in &selected_targets {
        endpoints_to_rebuild.insert(t.endpoint_id.clone());
    }

    for ep_id in endpoints_to_rebuild.iter() {
        let db_path = endpoint_index_db_path(data_dir, ep_id);
        let legacy_global = legacy_global_index_db_path(data_dir);
        let _ = legacy_global; // must not read/write legacy global db here (migration compat)

        // Build a replacement DB first, then swap it into place, so failures don't leave us without
        // `index.{endpoint_id}.sqlite`.
        let ts = config_bundle::utc_now_compact_timestamp();

        let backup_path = if db_path.exists() {
            let mut backup = db_path.clone();
            backup.set_extension(format!("sqlite.bak.{ts}"));
            if backup.exists() {
                backup.set_extension(format!("sqlite.bak.{ts}.1"));
            }
            Some(backup)
        } else {
            None
        };

        let mut tmp_path = db_path.clone();
        tmp_path.set_extension(format!("sqlite.tmp.{ts}"));
        if tmp_path.exists() {
            tmp_path.set_extension(format!("sqlite.tmp.{ts}.1"));
        }

        // Choose one target under this endpoint with remote latest available.
        let mut chosen_remote: Option<(String, String, String)> = None;
        if let Some(cat) = endpoint_catalogs.get(ep_id).and_then(|c| c.as_ref()) {
            for t in &selected_targets {
                if &t.endpoint_id != ep_id {
                    continue;
                }

                let mut latest = cat
                    .targets
                    .iter()
                    .find(|x| x.target_id == t.id)
                    .and_then(|x| x.latest.clone());
                if latest.is_none() {
                    let matches = cat
                        .targets
                        .iter()
                        .filter(|x| x.source_path == t.source_path)
                        .collect::<Vec<_>>();
                    if matches.len() == 1 {
                        latest = matches[0].latest.clone();
                    }
                }

                if let Some(latest) = latest {
                    chosen_remote =
                        Some((t.id.clone(), latest.snapshot_id, latest.manifest_object_id));
                    break;
                }
            }
        }

        if let Some((target_id, snapshot_id, manifest_object_id)) = chosen_remote {
            let provider = settings_config::endpoint_provider(ep_id);
            let storage = endpoint_storage.get(ep_id).ok_or_else(|| {
                CliError::retryable("telegram.unavailable", "telegram storage unavailable")
            })?;

            televy_backup_core::remote_index_db::download_and_write_index_db_atomic(
                storage,
                &snapshot_id,
                &manifest_object_id,
                &bundle_master_key,
                &tmp_path,
                None,
                Some(&provider),
            )
            .await
            .map_err(map_core_err)?;

            rebuilt_from = SettingsImportBundleApplyRebuiltFromJson {
                mode: "remote_latest".to_string(),
                snapshot_id: Some(snapshot_id),
                manifest_object_id: Some(manifest_object_id),
            };
            local_index_synced.push(SettingsImportBundleApplyLocalIndexSyncedJson {
                target_id,
                from: "remoteLatest".to_string(),
                to: "local".to_string(),
            });
        } else {
            // No bootstrap/latest: initialize an empty DB so future backups can build it up.
            init_empty_index_db(&tmp_path).await?;

            rebuilt_from = SettingsImportBundleApplyRebuiltFromJson {
                mode: "empty".to_string(),
                snapshot_id: None,
                manifest_object_id: None,
            };
        }

        // Swap: move old DB to a backup path (if any), then move the new DB into place.
        previous_backup_path = None;
        if let Some(backup) = &backup_path {
            if let Err(e) = std::fs::rename(&db_path, backup) {
                let _ = std::fs::remove_file(&tmp_path);
                return Err(CliError::new("config.write_failed", e.to_string()));
            }
            previous_backup_path = Some(backup.display().to_string());
        }

        if let Err(e) = std::fs::rename(&tmp_path, &db_path) {
            // Best-effort rollback: restore the previous DB if we moved it.
            if let Some(backup) = &backup_path {
                let _ = std::fs::rename(backup, &db_path);
            }
            let _ = std::fs::remove_file(&tmp_path);
            return Err(CliError::new("config.write_failed", e.to_string()));
        }

        rebuilt_db_path = db_path.display().to_string();
    }

    // Auto-clean legacy global DB if all in-use per-endpoint DBs are present and usable.
    let legacy_db_path = legacy_global_index_db_path(data_dir);
    if legacy_db_path.exists() {
        let mut in_use_endpoints = std::collections::BTreeSet::<String>::new();

        for t in &local_settings.targets {
            if t.enabled {
                in_use_endpoints.insert(t.endpoint_id.clone());
            }
        }
        for t in &selected_targets {
            in_use_endpoints.insert(t.endpoint_id.clone());
        }

        let mut all_ok = true;
        for ep_id in in_use_endpoints {
            let p = endpoint_index_db_path(data_dir, &ep_id);
            if !p.exists() {
                all_ok = false;
                break;
            }
            if televy_backup_core::index_db::open_index_db(&p)
                .await
                .is_err()
            {
                all_ok = false;
                break;
            }
        }

        if all_ok {
            let _ = std::fs::remove_file(&legacy_db_path);
        }
    }

    // Write settings (merge semantics) + secrets after index rebuild succeeds.
    let mut next_settings = local_settings.clone();
    next_settings.schedule = bundle_settings.schedule.clone();
    next_settings.retention = bundle_settings.retention.clone();
    next_settings.chunking = bundle_settings.chunking.clone();
    next_settings.telegram = bundle_settings.telegram.clone();

    let mut endpoints_written = std::collections::BTreeSet::<String>::new();
    for t in &selected_targets {
        endpoints_written.insert(t.endpoint_id.clone());

        // Upsert target.
        match next_settings.targets.iter_mut().find(|x| x.id == t.id) {
            Some(existing) => *existing = t.clone(),
            None => next_settings.targets.push(t.clone()),
        }
    }

    for ep_id in endpoints_written.iter() {
        if let Some(ep) = bundle_settings
            .telegram_endpoints
            .iter()
            .find(|e| &e.id == ep_id)
        {
            match next_settings
                .telegram_endpoints
                .iter_mut()
                .find(|x| x.id == ep.id)
            {
                Some(existing) => *existing = ep.clone(),
                None => next_settings.telegram_endpoints.push(ep.clone()),
            }
        }
    }

    settings_config::save_settings_v2(config_dir, &next_settings).map_err(map_core_err)?;

    let vault_key = load_or_create_vault_key(data_dir)?;
    let secrets_path = televy_backup_core::secrets::secrets_path(config_dir);
    let mut store = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
        .map_err(map_secrets_store_err)?;

    let mut secrets_written = Vec::new();
    let master_key_b64 = base64::engine::general_purpose::STANDARD.encode(bundle_master_key);
    store.set(MASTER_KEY_KEY, master_key_b64);
    secrets_written.push(MASTER_KEY_KEY.to_string());

    for (k, v) in bundle_secrets.entries.iter() {
        // Defense in depth: the core bundle decoder should reject this already.
        if k == MASTER_KEY_KEY {
            return Err(CliError::new(
                "config.invalid",
                "config bundle secrets must not contain televybackup.master_key",
            ));
        }
        store.set(k.as_str(), v.as_str());
        secrets_written.push(k.to_string());
    }

    televy_backup_core::secrets::save_secrets_store(&secrets_path, &vault_key, &store)
        .map_err(map_secrets_store_err)?;

    secrets_written.sort();
    secrets_written.dedup();

    let mut applied_targets = selected_targets
        .iter()
        .map(|t| t.id.clone())
        .collect::<Vec<_>>();
    applied_targets.sort();

    let mut applied_endpoints = endpoints_written.into_iter().collect::<Vec<_>>();
    applied_endpoints.sort();

    let resp = SettingsImportBundleApplyResponse {
        ok: true,
        local_index: SettingsImportBundleApplyLocalIndexJson {
            previous_db_backup_path: previous_backup_path,
            rebuilt_db_path,
            rebuilt_from,
        },
        applied: SettingsImportBundleApplyAppliedJson {
            targets: applied_targets,
            endpoints: applied_endpoints,
            secrets_written,
        },
        actions: SettingsImportBundleApplyActionsJson {
            updated_pinned_catalog: updated_pins,
            local_index_synced,
        },
    };

    println!(
        "{}",
        serde_json::to_string(&resp).map_err(|e| CliError::new("config.invalid", e.to_string()))?
    );

    Ok(())
}

fn select_endpoint<'a>(
    settings: &'a Settings,
    endpoint_id: Option<&str>,
) -> Result<&'a settings_config::TelegramEndpoint, CliError> {
    if settings.telegram_endpoints.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "no telegram endpoints configured",
        ));
    }

    if let Some(id) = endpoint_id {
        return settings
            .telegram_endpoints
            .iter()
            .find(|e| e.id == id)
            .ok_or_else(|| CliError::new("config.invalid", format!("unknown endpoint_id: {id}")));
    }

    if settings.telegram_endpoints.len() == 1 {
        return Ok(&settings.telegram_endpoints[0]);
    }
    if let Some(ep) = settings
        .telegram_endpoints
        .iter()
        .find(|e| e.id == "default")
    {
        return Ok(ep);
    }

    Err(CliError::new(
        "config.invalid",
        "multiple endpoints configured; pass --endpoint-id",
    ))
}

fn select_target<'a>(
    settings: &'a Settings,
    target_id: Option<&str>,
    source: Option<&Path>,
) -> Result<&'a settings_config::Target, CliError> {
    if settings.targets.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "no backup targets configured",
        ));
    }

    if let Some(id) = target_id {
        return settings
            .targets
            .iter()
            .find(|t| t.id == id)
            .ok_or_else(|| CliError::new("config.invalid", format!("unknown target_id: {id}")));
    }

    let Some(source) = source else {
        return Err(CliError::new(
            "config.invalid",
            "either --target-id or --source must be provided",
        ));
    };

    let source_str = source
        .to_str()
        .ok_or_else(|| CliError::new("config.invalid", "source path is not valid utf-8"))?;

    let mut matches = settings
        .targets
        .iter()
        .filter(|t| t.source_path == source_str)
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            format!("no target configured for source_path: {source_str}"),
        ));
    }
    if matches.len() > 1 {
        return Err(CliError::new(
            "config.invalid",
            format!("multiple targets match source_path={source_str}; use --target-id"),
        ));
    }
    Ok(matches.remove(0))
}

async fn secrets_set_telegram_bot_token(
    config_dir: &Path,
    data_dir: &Path,
    endpoint_id: Option<String>,
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let ep = select_endpoint(&settings, endpoint_id.as_deref())?;

    let mut token = String::new();
    std::io::stdin()
        .read_to_string(&mut token)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(CliError::new("config.invalid", "token is empty"));
    }
    daemon_control_secrets_set_telegram_bot_token(data_dir, &ep.id, &token)?;

    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn secrets_set_telegram_api_hash(
    config_dir: &Path,
    data_dir: &Path,
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let mut api_hash = String::new();
    std::io::stdin()
        .read_to_string(&mut api_hash)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let api_hash = api_hash.trim().to_string();
    if api_hash.is_empty() {
        return Err(CliError::new("config.invalid", "api_hash is empty"));
    }
    let _ = settings;
    daemon_control_secrets_set_telegram_api_hash(data_dir, &api_hash)?;

    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn secrets_clear_telegram_mtproto_session(
    config_dir: &Path,
    data_dir: &Path,
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    for ep in &settings.telegram_endpoints {
        daemon_control_secrets_clear_telegram_mtproto_session(data_dir, &ep.id)?;
    }

    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn secrets_migrate_keychain(
    config_dir: &Path,
    data_dir: &Path,
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;

    let mut migrated = Vec::<String>::new();
    let mut deleted = Vec::<String>::new();
    let mut conflicts = Vec::<String>::new();

    for ep in &settings.telegram_endpoints {
        let bot_key = ep.bot_token_key.clone();
        if let Some(token) = daemon_keychain_get_secret(data_dir, &bot_key)? {
            let store_val = get_secret(config_dir, data_dir, &bot_key)?;
            match store_val {
                None => {
                    set_secret(config_dir, data_dir, &bot_key, &token)?;
                    migrated.push(bot_key.clone());
                    if daemon_keychain_delete_secret(data_dir, &bot_key)? {
                        deleted.push(bot_key.clone());
                    }
                }
                Some(existing) => {
                    if existing == token {
                        if daemon_keychain_delete_secret(data_dir, &bot_key)? {
                            deleted.push(bot_key.clone());
                        }
                    } else {
                        conflicts.push(bot_key.clone());
                    }
                }
            }
        }
    }

    if let Some(master_key) = daemon_keychain_get_secret(data_dir, MASTER_KEY_KEY)? {
        let store_val = get_secret(config_dir, data_dir, MASTER_KEY_KEY)?;
        match store_val {
            None => {
                set_secret(config_dir, data_dir, MASTER_KEY_KEY, &master_key)?;
                migrated.push(MASTER_KEY_KEY.to_string());
                if daemon_keychain_delete_secret(data_dir, MASTER_KEY_KEY)? {
                    deleted.push(MASTER_KEY_KEY.to_string());
                }
            }
            Some(existing) => {
                if existing == master_key {
                    if daemon_keychain_delete_secret(data_dir, MASTER_KEY_KEY)? {
                        deleted.push(MASTER_KEY_KEY.to_string());
                    }
                } else {
                    return Err(CliError::new(
                        "secrets.migrate_conflict",
                        "master key differs between secrets store and Keychain; refusing to delete Keychain item. Fix: decide which master key to keep, then re-run migration.",
                    ));
                }
            }
        }
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "migrated": migrated,
                "deletedKeychainItems": deleted,
                "conflicts": conflicts,
            })
        );
    } else {
        println!("ok");
        if !conflicts.is_empty() {
            println!("conflicts={}", conflicts.join(","));
        }
    }

    Ok(())
}

async fn secrets_init_master_key(
    config_dir: &Path,
    data_dir: &Path,
    json: bool,
) -> Result<(), CliError> {
    if get_secret(config_dir, data_dir, MASTER_KEY_KEY)?.is_some() {
        return Err(CliError::new(
            "secrets.store_failed",
            "master key already exists",
        ));
    }

    if daemon_keychain_get_secret(data_dir, MASTER_KEY_KEY)?.is_some() {
        return Err(CliError::new(
            "secrets.store_failed",
            "master key exists in Keychain (old scheme). Fix: run `televybackup secrets migrate-keychain` instead of generating a new one.",
        ));
    }

    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| CliError::new("secrets.store_failed", format!("getrandom failed: {e}")))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    set_secret(config_dir, data_dir, MASTER_KEY_KEY, &b64)?;
    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn secrets_export_master_key(
    config_dir: &Path,
    data_dir: &Path,
    i_understand: bool,
    json: bool,
) -> Result<(), CliError> {
    if !i_understand {
        return Err(CliError::new(
            "config.invalid",
            "refusing to export master key without --i-understand",
        ));
    }

    let master_key = load_master_key(config_dir, data_dir)?;
    let gold = gold_key::encode_gold_key(&master_key);

    if json {
        println!(
            "{}",
            serde_json::json!({ "goldKey": gold, "format": gold_key::GOLD_KEY_FORMAT })
        );
    } else {
        println!("{gold}");
    }
    Ok(())
}

async fn secrets_import_master_key(
    config_dir: &Path,
    data_dir: &Path,
    force: bool,
    json: bool,
) -> Result<(), CliError> {
    if get_secret(config_dir, data_dir, MASTER_KEY_KEY)?.is_some() && !force {
        return Err(CliError::new(
            "secrets.store_failed",
            "master key already exists (pass --force to overwrite)",
        ));
    }

    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let input = input.trim();
    if input.is_empty() {
        return Err(CliError::new("config.invalid", "gold key is empty"));
    }

    let master_key = gold_key::decode_gold_key(input).map_err(map_core_err)?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(master_key);
    set_secret(config_dir, data_dir, MASTER_KEY_KEY, &b64)?;

    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn telegram_validate(
    config_dir: &Path,
    data_dir: &Path,
    endpoint_id: Option<String>,
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let ep = select_endpoint(&settings, endpoint_id.as_deref())?;

    if settings.telegram.mtproto.api_id <= 0 {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mtproto.api_id must be > 0",
        ));
    }
    if settings.telegram.mtproto.api_hash_key.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mtproto.api_hash_key must not be empty",
        ));
    }
    if ep.chat_id.trim().is_empty() {
        return Err(CliError::new(
            "config.invalid",
            format!("telegram_endpoints[{id}].chat_id is empty", id = ep.id),
        ));
    }

    let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let api_hash = get_secret(
        config_dir,
        data_dir,
        &settings.telegram.mtproto.api_hash_key,
    )?
    .ok_or_else(|| {
        CliError::new(
            "telegram.mtproto.missing_api_hash",
            "mtproto api_hash missing",
        )
    })?;

    let session = load_optional_base64_secret_bytes(
        config_dir,
        data_dir,
        &ep.mtproto.session_key,
        "telegram.mtproto.session_invalid",
        "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
    )?;

    let cache_dir = data_dir.join("cache").join("mtproto");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

    let provider = settings_config::endpoint_provider(&ep.id);

    let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
        provider,
        api_id: settings.telegram.mtproto.api_id,
        api_hash: api_hash.clone(),
        bot_token: bot_token.clone(),
        chat_id: ep.chat_id.clone(),
        session,
        cache_dir,
        helper_path: None,
    })
    .await
    .map_err(|e| map_mtproto_validate_err(e, &bot_token, &api_hash))?;

    let mut sample = vec![0u8; 1024];
    getrandom::getrandom(&mut sample)
        .map_err(|e| CliError::new("config.invalid", format!("getrandom failed: {e}")))?;

    let sample_name = "televybackup-validate.bin";
    let object_id = storage
        .upload_document(sample_name, sample.clone())
        .await
        .map_err(|e| map_mtproto_validate_err(e, &bot_token, &api_hash))?;
    let downloaded = storage
        .download_document(&object_id)
        .await
        .map_err(|e| map_mtproto_validate_err(e, &bot_token, &api_hash))?;
    if downloaded != sample {
        return Err(CliError::new(
            "telegram.roundtrip_failed",
            format!(
                "roundtrip mismatch: uploaded_len={} downloaded_len={}",
                sample.len(),
                downloaded.len()
            ),
        ));
    }

    let session = storage.session_bytes();
    if let Some(bytes) = session {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64)?;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "mode": "mtproto",
                "endpointId": ep.id,
                "chatId": ep.chat_id,
                "roundTripOk": true,
                "sampleObjectId": object_id,
            })
        );
    } else {
        println!("mode=mtproto");
        println!("endpointId={}", ep.id);
        println!("chatId={}", ep.chat_id);
        println!("roundTripOk=true");
        println!("sampleObjectId={object_id}");
    }
    Ok(())
}

async fn telegram_dialogs(
    config_dir: &Path,
    data_dir: &Path,
    endpoint_id: Option<String>,
    limit: u32,
    include_users: bool,
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let ep = select_endpoint(&settings, endpoint_id.as_deref())?;

    if settings.telegram.mtproto.api_id <= 0 {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mtproto.api_id must be > 0",
        ));
    }
    if settings.telegram.mtproto.api_hash_key.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mtproto.api_hash_key must not be empty",
        ));
    }

    let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let api_hash = get_secret(
        config_dir,
        data_dir,
        &settings.telegram.mtproto.api_hash_key,
    )?
    .ok_or_else(|| {
        CliError::new(
            "telegram.mtproto.missing_api_hash",
            "mtproto api_hash missing",
        )
    })?;
    let session = load_optional_base64_secret_bytes(
        config_dir,
        data_dir,
        &ep.mtproto.session_key,
        "telegram.mtproto.session_invalid",
        "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
    )?;

    let cache_dir = data_dir.join("cache").join("mtproto");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

    let provider = settings_config::endpoint_provider(&ep.id);
    let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
        provider,
        api_id: settings.telegram.mtproto.api_id,
        api_hash: api_hash.clone(),
        bot_token: bot_token.clone(),
        // Dialog listing does not require a selected chat. Intentionally skip resolve_chat so users
        // can discover a valid group/channel even when chat_id is empty/invalid.
        chat_id: String::new(),
        session,
        cache_dir,
        helper_path: None,
    })
    .await
    .map_err(map_core_err)?;

    let limit = limit.clamp(1, 5_000) as usize;
    let dialogs = match storage.list_dialogs(limit, include_users) {
        Ok(v) => v,
        Err(televy_backup_core::Error::Telegram { message })
            if message.contains("BOT_METHOD_INVALID")
                || message.contains("messages.getDialogs") =>
        {
            return Err(CliError::new(
                "telegram.dialogs_unsupported",
                "bots cannot list dialogs via MTProto (messages.getDialogs rejected); use `televybackup telegram wait-chat` and send a message in the target group/channel to discover its chat_id",
            ));
        }
        Err(e) => return Err(map_core_err(e)),
    };

    if let Some(bytes) = storage.session_bytes() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        if let Err(e) = set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64) {
            tracing::warn!(
                event = "secrets.session_persist_failed",
                error_code = e.code,
                error_message = %e.message,
                "failed to persist mtproto session"
            );
        }
    }

    if json {
        let items = dialogs
            .iter()
            .map(|d| {
                serde_json::json!({
                    "kind": d.kind,
                    "title": d.title,
                    "username": d.username,
                    "peerId": d.peer_id,
                    "configChatId": d.config_chat_id,
                    "bootstrapHint": d.bootstrap_hint,
                })
            })
            .collect::<Vec<_>>();
        println!("{}", serde_json::json!({ "dialogs": items }));
        return Ok(());
    }

    for d in dialogs {
        let u = d
            .username
            .as_ref()
            .map(|s| format!("@{s}"))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "kind={} chatId={} username={} title={}",
            d.kind, d.config_chat_id, u, d.title
        );
    }
    Ok(())
}

async fn telegram_wait_chat(
    config_dir: &Path,
    data_dir: &Path,
    endpoint_id: Option<String>,
    timeout_secs: u32,
    include_users: bool,
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let ep = select_endpoint(&settings, endpoint_id.as_deref())?;

    if settings.telegram.mtproto.api_id <= 0 {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mtproto.api_id must be > 0",
        ));
    }
    if settings.telegram.mtproto.api_hash_key.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mtproto.api_hash_key must not be empty",
        ));
    }

    let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let api_hash = get_secret(
        config_dir,
        data_dir,
        &settings.telegram.mtproto.api_hash_key,
    )?
    .ok_or_else(|| {
        CliError::new(
            "telegram.mtproto.missing_api_hash",
            "mtproto api_hash missing",
        )
    })?;
    let session = load_optional_base64_secret_bytes(
        config_dir,
        data_dir,
        &ep.mtproto.session_key,
        "telegram.mtproto.session_invalid",
        "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
    )?;

    let cache_dir = data_dir.join("cache").join("mtproto");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

    let provider = settings_config::endpoint_provider(&ep.id);
    let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
        provider,
        api_id: settings.telegram.mtproto.api_id,
        api_hash: api_hash.clone(),
        bot_token: bot_token.clone(),
        // WaitChat does not require a selected chat.
        chat_id: String::new(),
        session,
        cache_dir,
        helper_path: None,
    })
    .await
    .map_err(map_core_err)?;

    let timeout_secs = (timeout_secs as u64).clamp(1, 10 * 60);
    let chat = match storage.wait_for_chat(timeout_secs, include_users) {
        Ok(v) => v,
        Err(televy_backup_core::Error::Telegram { message })
            if message.contains("wait_for_chat timed out") =>
        {
            return Err(CliError::retryable("telegram.timeout", message));
        }
        Err(e) => return Err(map_core_err(e)),
    };

    if let Some(bytes) = storage.session_bytes() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        if let Err(e) = set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64) {
            tracing::warn!(
                event = "secrets.session_persist_failed",
                error_code = e.code,
                error_message = %e.message,
                "failed to persist mtproto session"
            );
        }
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "chat": {
                    "kind": chat.kind,
                    "title": chat.title,
                    "username": chat.username,
                    "peerId": chat.peer_id,
                    "configChatId": chat.config_chat_id,
                    "bootstrapHint": chat.bootstrap_hint,
                }
            })
        );
        return Ok(());
    }

    let u = chat
        .username
        .as_ref()
        .map(|s| format!("@{s}"))
        .unwrap_or_else(|| "-".to_string());
    println!(
        "kind={} chatId={} username={} title={}",
        chat.kind, chat.config_chat_id, u, chat.title
    );
    Ok(())
}

fn load_optional_base64_secret_bytes(
    config_dir: &Path,
    data_dir: &Path,
    key: &str,
    error_code: &'static str,
    error_message: &str,
) -> Result<Option<Vec<u8>>, CliError> {
    let Some(b64) = get_secret(config_dir, data_dir, key)? else {
        return Ok(None);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| CliError::new(error_code, format!("{error_message}: {e}")))?;
    Ok(Some(bytes))
}

fn map_mtproto_validate_err(
    e: televy_backup_core::Error,
    bot_token: &str,
    api_hash: &str,
) -> CliError {
    let msg = redact_secret(redact_secret(e.to_string(), bot_token), api_hash);
    let msg_lc = msg.to_ascii_lowercase();

    if msg_lc.contains("invalid session base64") || msg_lc.contains("session load failed") {
        return CliError::new("telegram.mtproto.session_invalid", msg);
    }
    if msg_lc.contains("bot_sign_in failed") {
        return CliError::new("telegram.unauthorized", msg);
    }
    if msg_lc.contains("chat not found") || msg_lc.contains("resolve chat failed") {
        return CliError::new("telegram.chat_not_found", msg);
    }
    if msg_lc.contains("message not found") || msg_lc.contains("document mismatch") {
        return CliError::new("telegram.roundtrip_failed", msg);
    }

    match e {
        televy_backup_core::Error::Telegram { .. } => {
            CliError::retryable("telegram.unavailable", msg)
        }
        televy_backup_core::Error::InvalidConfig { .. } => {
            if msg_lc.contains("failed to start mtproto helper")
                || msg_lc.contains("mtproto helper missing stdin")
                || msg_lc.contains("mtproto helper missing stdout")
                || msg_lc.contains("cache dir create failed")
            {
                CliError::new("config.invalid", msg)
            } else {
                CliError::retryable("telegram.unavailable", msg)
            }
        }
        televy_backup_core::Error::Integrity { .. } => {
            CliError::new("telegram.roundtrip_failed", msg)
        }
        _ => CliError::new("telegram.roundtrip_failed", msg),
    }
}

fn list_index_db_paths_for_read(data_dir: &Path) -> Result<Vec<PathBuf>, CliError> {
    let index_dir = data_dir.join("index");
    let mut dbs = Vec::<PathBuf>::new();

    match std::fs::read_dir(&index_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_file() {
                    continue;
                }

                let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
                    continue;
                };

                // Legacy global DB is a fallback only; prefer per-endpoint DBs when present.
                if name == "index.sqlite" {
                    continue;
                }
                if !name.starts_with("index.") || !name.ends_with(".sqlite") {
                    continue;
                }

                dbs.push(path);
            }
        }
        Err(e) => {
            if e.kind() != std::io::ErrorKind::NotFound {
                return Err(CliError::new("db.failed", e.to_string()));
            }
        }
    }

    dbs.sort();
    if dbs.is_empty() {
        let legacy = legacy_global_index_db_path(data_dir);
        if legacy.exists() {
            dbs.push(legacy);
        }
    }

    Ok(dbs)
}

async fn snapshots_list(data_dir: &Path, limit: u32, json: bool) -> Result<(), CliError> {
    let db_paths = list_index_db_paths_for_read(data_dir)?;
    if db_paths.is_empty() {
        if json {
            println!("{}", serde_json::json!({ "snapshots": [] }));
        }
        return Ok(());
    }

    #[derive(Debug)]
    struct SnapshotListItem {
        snapshot_id: String,
        created_at: String,
        source_path: String,
        label: String,
        base_snapshot_id: Option<String>,
    }

    // Query each DB for its newest snapshots, then merge and keep the global top N.
    // This matches the legacy "single global DB" behavior while the index is now per-endpoint.
    let mut items: Vec<SnapshotListItem> = Vec::new();
    for db_path in db_paths {
        let pool = televy_backup_core::index_db::open_existing_index_db(&db_path)
            .await
            .map_err(map_core_err)?;

        let rows = sqlx::query(
            r#"
            SELECT snapshot_id, created_at, source_path, label, base_snapshot_id
            FROM snapshots
            ORDER BY created_at DESC
            LIMIT ?
            "#,
        )
        .bind(limit as i64)
        .fetch_all(&pool)
        .await
        .map_err(|e| CliError::new("db.failed", e.to_string()))?;

        for row in rows {
            items.push(SnapshotListItem {
                snapshot_id: row.get::<String, _>("snapshot_id"),
                created_at: row.get::<String, _>("created_at"),
                source_path: row.get::<String, _>("source_path"),
                label: row.get::<String, _>("label"),
                base_snapshot_id: row.get::<Option<String>, _>("base_snapshot_id"),
            });
        }
    }

    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    items.truncate(limit as usize);

    let out = items
        .into_iter()
        .map(|i| {
            serde_json::json!({
                "snapshotId": i.snapshot_id,
                "createdAt": i.created_at,
                "sourcePath": i.source_path,
                "label": i.label,
                "baseSnapshotId": i.base_snapshot_id,
            })
        })
        .collect::<Vec<_>>();

    if json {
        println!("{}", serde_json::json!({ "snapshots": out }));
    } else {
        for s in out {
            println!("{}", s);
        }
    }
    Ok(())
}

async fn stats_get(data_dir: &Path, json: bool) -> Result<(), CliError> {
    let db_paths = list_index_db_paths_for_read(data_dir)?;
    if db_paths.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({ "snapshotsTotal": 0, "chunksTotal": 0, "chunksBytesTotal": 0 })
            );
        }
        return Ok(());
    }

    let mut snapshots_total: i64 = 0;
    let mut chunks_total: i64 = 0;
    let mut chunks_bytes_total: i64 = 0;
    for db_path in db_paths {
        let pool = televy_backup_core::index_db::open_existing_index_db(&db_path)
            .await
            .map_err(map_core_err)?;

        snapshots_total = snapshots_total.saturating_add(
            sqlx::query("SELECT COUNT(1) as c FROM snapshots")
                .fetch_one(&pool)
                .await
                .map_err(|e| CliError::new("db.failed", e.to_string()))?
                .get::<i64, _>("c"),
        );
        chunks_total = chunks_total.saturating_add(
            sqlx::query("SELECT COUNT(1) as c FROM chunks")
                .fetch_one(&pool)
                .await
                .map_err(|e| CliError::new("db.failed", e.to_string()))?
                .get::<i64, _>("c"),
        );
        chunks_bytes_total = chunks_bytes_total.saturating_add(
            sqlx::query("SELECT COALESCE(SUM(size), 0) as s FROM chunks")
                .fetch_one(&pool)
                .await
                .map_err(|e| CliError::new("db.failed", e.to_string()))?
                .get::<i64, _>("s"),
        );
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "snapshotsTotal": snapshots_total,
                "chunksTotal": chunks_total,
                "chunksBytesTotal": chunks_bytes_total
            })
        );
    } else {
        println!("snapshotsTotal={snapshots_total}");
        println!("chunksTotal={chunks_total}");
        println!("chunksBytesTotal={chunks_bytes_total}");
    }
    Ok(())
}

async fn stats_last(data_dir: &Path, source: Option<PathBuf>, json: bool) -> Result<(), CliError> {
    let db_paths = list_index_db_paths_for_read(data_dir)?;
    if db_paths.is_empty() {
        if json {
            println!(
                "{}",
                serde_json::json!({ "snapshot": serde_json::Value::Null })
            );
        }
        return Ok(());
    }

    let source_str: Option<String> = source
        .as_ref()
        .map(|p| {
            p.to_str()
                .ok_or_else(|| CliError::new("config.invalid", "source path is not valid utf-8"))
                .map(|s| s.to_string())
        })
        .transpose()?;

    #[derive(Debug)]
    struct LastSnapshot {
        db_path: PathBuf,
        snapshot_id: String,
        created_at: String,
        base_snapshot_id: Option<String>,
    }

    let mut best: Option<LastSnapshot> = None;
    for db_path in db_paths {
        let pool = televy_backup_core::index_db::open_existing_index_db(&db_path)
            .await
            .map_err(map_core_err)?;

        let snapshot_row: Option<sqlx::sqlite::SqliteRow> = if let Some(source) = &source_str {
            sqlx::query(
                r#"
                SELECT snapshot_id, created_at, base_snapshot_id
                FROM snapshots
                WHERE source_path = ?
                ORDER BY created_at DESC
                LIMIT 1
                "#,
            )
            .bind(source)
            .fetch_optional(&pool)
            .await
            .map_err(|e| CliError::new("db.failed", e.to_string()))?
        } else {
            sqlx::query(
                r#"
                SELECT snapshot_id, created_at, base_snapshot_id
                FROM snapshots
                ORDER BY created_at DESC
                LIMIT 1
                "#,
            )
            .fetch_optional(&pool)
            .await
            .map_err(|e| CliError::new("db.failed", e.to_string()))?
        };

        let Some(row) = snapshot_row else {
            continue;
        };

        let snapshot_id: String = row
            .try_get("snapshot_id")
            .map_err(|e| CliError::new("db.failed", e.to_string()))?;
        let created_at: String = row
            .try_get("created_at")
            .map_err(|e| CliError::new("db.failed", e.to_string()))?;
        let base_snapshot_id: Option<String> = row
            .try_get("base_snapshot_id")
            .map_err(|e| CliError::new("db.failed", e.to_string()))?;

        let candidate = LastSnapshot {
            db_path: db_path.clone(),
            snapshot_id,
            created_at,
            base_snapshot_id,
        };

        let replace = match best.as_ref() {
            None => true,
            Some(cur) => candidate.created_at > cur.created_at,
        };
        if replace {
            best = Some(candidate);
        }
    }

    let Some(best) = best else {
        if json {
            println!(
                "{}",
                serde_json::json!({ "snapshot": serde_json::Value::Null })
            );
        }
        return Ok(());
    };

    let snapshot_id = best.snapshot_id;
    let created_at = best.created_at;
    let base_snapshot_id = best.base_snapshot_id;

    let pool = televy_backup_core::index_db::open_existing_index_db(&best.db_path)
        .await
        .map_err(map_core_err)?;

    let cur_bytes_unique: i64 = sqlx::query(
        r#"
        SELECT COALESCE(SUM(c.size), 0) AS s
        FROM chunks c
        JOIN (
          SELECT DISTINCT fc.chunk_hash AS chunk_hash
          FROM file_chunks fc
          JOIN files f ON f.file_id = fc.file_id
          WHERE f.snapshot_id = ?
        ) x ON x.chunk_hash = c.chunk_hash
        "#,
    )
    .bind(&snapshot_id)
    .fetch_one(&pool)
    .await
    .map_err(|e| CliError::new("db.failed", e.to_string()))?
    .get("s");

    let bytes_new: i64 = if let Some(base_id) = &base_snapshot_id {
        sqlx::query(
            r#"
            WITH cur AS (
              SELECT DISTINCT fc.chunk_hash AS chunk_hash
              FROM file_chunks fc
              JOIN files f ON f.file_id = fc.file_id
              WHERE f.snapshot_id = ?
            ),
            base AS (
              SELECT DISTINCT fc.chunk_hash AS chunk_hash
              FROM file_chunks fc
              JOIN files f ON f.file_id = fc.file_id
              WHERE f.snapshot_id = ?
            ),
            new_only AS (
              SELECT cur.chunk_hash AS chunk_hash
              FROM cur
              LEFT JOIN base ON base.chunk_hash = cur.chunk_hash
              WHERE base.chunk_hash IS NULL
            )
            SELECT COALESCE(SUM(c.size), 0) AS s
            FROM chunks c
            JOIN new_only n ON n.chunk_hash = c.chunk_hash
            "#,
        )
        .bind(&snapshot_id)
        .bind(base_id)
        .fetch_one(&pool)
        .await
        .map_err(|e| CliError::new("db.failed", e.to_string()))?
        .get("s")
    } else {
        cur_bytes_unique
    };

    let bytes_reused = (cur_bytes_unique - bytes_new).max(0);

    let duration_seconds: Option<f64> = sqlx::query(
        r#"
        SELECT (julianday(ri.created_at) - julianday(s.created_at)) * 86400.0 AS seconds
        FROM snapshots s
        JOIN remote_indexes ri ON ri.snapshot_id = s.snapshot_id
        WHERE s.snapshot_id = ?
        LIMIT 1
        "#,
    )
    .bind(&snapshot_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| CliError::new("db.failed", e.to_string()))?
    .map(|r| r.get::<f64, _>("seconds"));

    if json {
        println!(
            "{}",
            serde_json::json!({
                "snapshot": {
                    "snapshotId": snapshot_id,
                    "createdAt": created_at,
                    "baseSnapshotId": base_snapshot_id,
                    "bytesUploaded": bytes_new,
                    "bytesDeduped": bytes_reused,
                    "durationSeconds": duration_seconds,
                    "approx": true
                }
            })
        );
    } else {
        println!("snapshotId={snapshot_id}");
        println!("createdAt={created_at}");
        println!("bytesUploaded={bytes_new}");
        println!("bytesDeduped={bytes_reused}");
        if let Some(s) = duration_seconds {
            println!("durationSeconds={s}");
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn backup_run(
    config_dir: &Path,
    data_dir: &Path,
    target_id: Option<String>,
    source: Option<PathBuf>,
    label: String,
    no_remote_index_sync: bool,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
    let run_log = televy_backup_core::run_log::start_run_log("backup", &task_id, data_dir)
        .map_err(|e| CliError::new("log.init_failed", e.to_string()))?;

    let started = std::time::Instant::now();

    let settings = match load_settings(config_dir) {
        Ok(s) => s,
        Err(e) => {
            return emit_preflight_failed(
                events,
                &task_id,
                "backup",
                run_log.path(),
                started,
                RunCtx {
                    target_id: None,
                    endpoint_id: None,
                    source_path: None,
                    snapshot_id: None,
                },
                e,
            );
        }
    };

    let target = match select_target(&settings, target_id.as_deref(), source.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            return emit_preflight_failed(
                events,
                &task_id,
                "backup",
                run_log.path(),
                started,
                RunCtx {
                    target_id: None,
                    endpoint_id: None,
                    source_path: None,
                    snapshot_id: None,
                },
                e,
            );
        }
    };

    let ep = match settings
        .telegram_endpoints
        .iter()
        .find(|e| e.id == target.endpoint_id)
    {
        Some(ep) => ep,
        None => {
            let e = CliError::new(
                "config.invalid",
                format!(
                    "target references unknown endpoint_id: target_id={} endpoint_id={}",
                    target.id, target.endpoint_id
                ),
            );
            return emit_preflight_failed(
                events,
                &task_id,
                "backup",
                run_log.path(),
                started,
                RunCtx {
                    target_id: Some(target.id.as_str()),
                    endpoint_id: None,
                    source_path: Some(target.source_path.as_str()),
                    snapshot_id: None,
                },
                e,
            );
        }
    };

    if settings.telegram.mtproto.api_id <= 0 {
        let e = CliError::new("config.invalid", "telegram.mtproto.api_id must be > 0");
        return emit_preflight_failed(
            events,
            &task_id,
            "backup",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(target.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(target.source_path.as_str()),
                snapshot_id: None,
            },
            e,
        );
    }
    if settings.telegram.mtproto.api_hash_key.is_empty() {
        let e = CliError::new(
            "config.invalid",
            "telegram.mtproto.api_hash_key must not be empty",
        );
        return emit_preflight_failed(
            events,
            &task_id,
            "backup",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(target.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(target.source_path.as_str()),
                snapshot_id: None,
            },
            e,
        );
    }
    if ep.chat_id.is_empty() {
        let e = CliError::new(
            "config.invalid",
            format!("telegram_endpoints[{id}].chat_id is empty", id = ep.id),
        );
        return emit_preflight_failed(
            events,
            &task_id,
            "backup",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(target.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(target.source_path.as_str()),
                snapshot_id: None,
            },
            e,
        );
    }

    let ctx_target_id = target.id.clone();
    let ctx_endpoint_id = ep.id.clone();
    let ctx_source_path = target.source_path.clone();

    // Run summaries must appear even when the CLI is started with `RUST_LOG=warn`,
    // otherwise successful runs create empty NDJSON files and the UI shows no history.
    tracing::warn!(
        event = "run.start",
        kind = "backup",
        run_id = %task_id,
        task_id = %task_id,
        target_id = %ctx_target_id,
        endpoint_id = %ctx_endpoint_id,
        source_path = %ctx_source_path,
        log_path = %run_log.path().display(),
        "run.start"
    );

    emit_task_state_running(
        events,
        &task_id,
        "backup",
        Some(ctx_target_id.as_str()),
        None,
    );
    emit_task_progress_preflight(events, &task_id);

    let result: Result<televy_backup_core::BackupResult, CliError> = async {
        let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
            .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
        let master_key = load_master_key(config_dir, data_dir)?;

        let db_path = endpoint_index_db_path(data_dir, &ep.id);
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
            throttle: Mutex::new(ProgressThrottle::new(Duration::from_millis(200))),
        };
        let opts = BackupOptions {
            cancel: None,
            progress: if events { Some(&sink) } else { None },
        };

        let cfg = BackupConfig {
            db_path: db_path.clone(),
            source_path: PathBuf::from(target.source_path.clone()),
            label,
            chunking: ChunkingConfig {
                min_bytes: settings.chunking.min_bytes,
                avg_bytes: settings.chunking.avg_bytes,
                max_bytes: settings.chunking.max_bytes,
            },
            rate_limit: ep.rate_limit.clone(),
            master_key,
            snapshot_id: None,
            keep_last_snapshots: settings.retention.keep_last_snapshots,
        };
        let label_for_bootstrap = cfg.label.clone();

        let api_hash = get_secret(config_dir, data_dir, &settings.telegram.mtproto.api_hash_key)?
            .ok_or_else(|| CliError::new("telegram.mtproto.missing_api_hash", "mtproto api_hash missing"))?;
        let session = load_optional_base64_secret_bytes(
            config_dir,
            data_dir,
            &ep.mtproto.session_key,
            "telegram.mtproto.session_invalid",
            "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
        )?;

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let provider = settings_config::endpoint_provider(&ep.id);
        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            provider,
            api_id: settings.telegram.mtproto.api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: ep.chat_id.clone(),
            session,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(map_core_err)?;

        preflight_remote_first_index_sync(
            &storage,
            &master_key,
            &target.id,
            &target.source_path,
            &db_path,
            no_remote_index_sync,
            is_likely_private_chat_id(&ep.chat_id),
            events.then_some(&sink),
        )
        .await?;

        let res = run_backup_with(&storage, cfg, opts)
            .await
            .map_err(map_core_err)?;

        // Update remote bootstrap/catalog for cross-device restore. This uses Telegram pinned
        // messages; if chat_id points at a private user dialog, MTProto bots can't rely on pinning.
        if no_remote_index_sync {
            tracing::warn!(
                event = "bootstrap.skipped",
                reason = "flag_no_remote_index_sync",
                "skipping bootstrap catalog update (--no-remote-index-sync)"
            );
        } else if is_likely_private_chat_id(&ep.chat_id) {
            tracing::warn!(
                event = "bootstrap.skipped",
                reason = "unsupported_private_chat",
                chat_id = %ep.chat_id,
                "bootstrap catalog requires pinning; use a group/channel (e.g. -100...) or @username chat id"
            );
        } else {
            let (manifest_object_id, snapshot_provider) =
                lookup_manifest_meta(&db_path, &res.snapshot_id).await?;
            if snapshot_provider != storage.provider() {
                return Err(CliError::new(
                    "snapshot.unsupported_provider",
                    format!(
                        "unexpected snapshot provider in local db: snapshot_id={} expected_provider={} got_provider={}",
                        res.snapshot_id,
                        storage.provider(),
                        snapshot_provider
                    ),
                ));
            }
            televy_backup_core::bootstrap::update_remote_latest(
                &storage,
                &master_key,
                &target.id,
                &target.source_path,
                &label_for_bootstrap,
                &res.snapshot_id,
                &manifest_object_id,
            )
            .await
            .map_err(map_core_err)?;
        }

        if let Some(bytes) = storage.session_bytes() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            if let Err(e) = set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64) {
                tracing::warn!(
                    event = "secrets.session_persist_failed",
                    error_code = e.code,
                    error_message = %e.message,
                    "failed to persist mtproto session"
                );
            }
        }

        Ok(res)
    }
    .await;

    let duration_seconds = started.elapsed().as_secs_f64();
    match result {
        Ok(res) => {
            tracing::warn!(
                event = "run.finish",
                kind = "backup",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %ctx_target_id,
                endpoint_id = %ctx_endpoint_id,
                source_path = %ctx_source_path,
                status = "succeeded",
                duration_seconds,
                snapshot_id = %res.snapshot_id,
                files_indexed = res.files_indexed,
                chunks_uploaded = res.chunks_uploaded,
                data_objects_uploaded = res.data_objects_uploaded,
                data_objects_estimated_without_pack = res.data_objects_estimated_without_pack,
                bytes_uploaded = res.bytes_uploaded,
                bytes_deduped = res.bytes_deduped,
                index_parts = res.index_parts,
                "run.finish"
            );

            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "backup",
                    "state": "succeeded",
                    "snapshotId": res.snapshot_id,
                    "targetId": ctx_target_id,
                    "result": {
                        "filesIndexed": res.files_indexed,
                        "chunksUploaded": res.chunks_uploaded,
                        "dataObjectsUploaded": res.data_objects_uploaded,
                        "dataObjectsEstimatedWithoutPack": res.data_objects_estimated_without_pack,
                        "bytesUploaded": res.bytes_uploaded,
                        "bytesDeduped": res.bytes_deduped,
                        "indexParts": res.index_parts,
                        "durationSeconds": duration_seconds,
                    }
                }));
                return Ok(());
            }

            if json {
                println!(
                    "{}",
                    serde_json::to_string(&res)
                        .map_err(|e| CliError::new("config.invalid", e.to_string()))?
                );
            } else {
                println!("snapshotId={}", res.snapshot_id);
                println!(
                    "filesIndexed={} chunksUploaded={} dataObjectsUploaded={} dataObjectsEstimatedWithoutPack={} bytesUploaded={} bytesDeduped={}",
                    res.files_indexed,
                    res.chunks_uploaded,
                    res.data_objects_uploaded,
                    res.data_objects_estimated_without_pack,
                    res.bytes_uploaded,
                    res.bytes_deduped
                );
            }
            Ok(())
        }
        Err(e) => {
            tracing::error!(
                event = "run.finish",
                kind = "backup",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %ctx_target_id,
                endpoint_id = %ctx_endpoint_id,
                source_path = %ctx_source_path,
                status = "failed",
                duration_seconds,
                error_code = e.code,
                error_message = %e.message,
                retryable = e.retryable,
                "run.finish"
            );
            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "backup",
                    "state": "failed",
                    "targetId": ctx_target_id,
                    "error": { "code": e.code, "message": e.message.clone() },
                }));
            }
            Err(e)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn preflight_remote_first_index_sync(
    storage: &TelegramMtProtoStorage,
    master_key: &[u8; 32],
    target_id: &str,
    source_path: &str,
    local_index_db: &Path,
    no_remote_index_sync: bool,
    is_private_chat: bool,
    sink: Option<&dyn ProgressSink>,
) -> Result<(), CliError> {
    if no_remote_index_sync {
        return Ok(());
    }

    let started = Instant::now();
    tracing::debug!(event = "phase.start", phase = "index_sync", "phase.start");
    if let Some(sink) = sink {
        sink.on_progress(televy_backup_core::TaskProgress {
            phase: "index_sync".to_string(),
            ..Default::default()
        });
    }

    if is_private_chat {
        tracing::warn!(
            event = "index_sync.skipped",
            reason = "unsupported_private_chat",
            "index_sync requires pinned bootstrap catalog; use a group/channel (e.g. -100...) or @username chat id"
        );
        tracing::debug!(
            event = "phase.finish",
            phase = "index_sync",
            duration_ms = started.elapsed().as_millis() as u64,
            index_source = "skipped",
            reason = "unsupported_chat",
            "phase.finish"
        );
        return Ok(());
    }

    let catalog = televy_backup_core::bootstrap::load_remote_catalog(storage, master_key)
        .await
        .map_err(map_core_err)?;

    let Some(catalog) = catalog else {
        tracing::debug!(
            event = "phase.finish",
            phase = "index_sync",
            duration_ms = started.elapsed().as_millis() as u64,
            index_source = "skipped",
            reason = "bootstrap_missing",
            "phase.finish"
        );
        return Ok(());
    };

    let latest = if let Some(t) = catalog.targets.iter().find(|t| t.target_id == target_id) {
        t.latest.clone().ok_or_else(|| {
            CliError::new(
                "config.invalid",
                format!("bootstrap missing latest for target_id: {target_id}"),
            )
        })?
    } else {
        let matches = catalog
            .targets
            .iter()
            .filter(|t| t.source_path == source_path)
            .collect::<Vec<_>>();

        if matches.is_empty() {
            return Err(CliError::new(
                "config.invalid",
                format!("bootstrap missing source_path: {source_path}"),
            ));
        }
        if matches.len() > 1 {
            return Err(CliError::new(
                "config.invalid",
                format!("bootstrap source_path is ambiguous: {source_path}"),
            ));
        }
        matches[0].latest.clone().ok_or_else(|| {
            CliError::new(
                "config.invalid",
                format!("bootstrap missing latest for source_path: {source_path}"),
            )
        })?
    };

    let provider = storage.provider();
    let already_synced = televy_backup_core::index_sync::local_index_matches_remote_latest(
        local_index_db,
        provider,
        &latest.snapshot_id,
        &latest.manifest_object_id,
    )
    .await
    .map_err(map_core_err)?;

    if already_synced {
        tracing::debug!(
            event = "phase.finish",
            phase = "index_sync",
            duration_ms = started.elapsed().as_millis() as u64,
            index_source = "skipped",
            reason = "already_synced",
            "phase.finish"
        );
        return Ok(());
    }

    let stats = televy_backup_core::remote_index_db::download_and_write_index_db_atomic(
        storage,
        &latest.snapshot_id,
        &latest.manifest_object_id,
        master_key,
        local_index_db,
        None,
        Some(provider),
    )
    .await
    .map_err(map_core_err)?;

    tracing::debug!(
        event = "phase.finish",
        phase = "index_sync",
        duration_ms = started.elapsed().as_millis() as u64,
        index_source = "downloaded",
        bytes_downloaded = stats.bytes_downloaded,
        bytes_written = stats.bytes_written,
        snapshot_id = %latest.snapshot_id,
        "phase.finish"
    );

    Ok(())
}

fn is_likely_private_chat_id(chat_id: &str) -> bool {
    let s = chat_id.trim();
    let Ok(id) = s.parse::<i64>() else {
        return false;
    };
    id > 0
}

async fn restore_run(
    config_dir: &Path,
    data_dir: &Path,
    snapshot_id: String,
    target: PathBuf,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
    let run_log = televy_backup_core::run_log::start_run_log("restore", &task_id, data_dir)
        .map_err(|e| CliError::new("log.init_failed", e.to_string()))?;

    tracing::warn!(
        event = "run.start",
        kind = "restore",
        run_id = %task_id,
        task_id = %task_id,
        snapshot_id = %snapshot_id,
        log_path = %run_log.path().display(),
        "run.start"
    );

    let started = std::time::Instant::now();
    let result: Result<televy_backup_core::RestoreResult, CliError> = async {
        let settings = load_settings(config_dir)?;

        let (manifest_object_id, snapshot_provider) =
            lookup_manifest_meta_any(data_dir, &snapshot_id).await?;

        let endpoint_id = if snapshot_provider == "telegram.mtproto" {
            None
        } else if let Some(rest) = snapshot_provider.strip_prefix("telegram.mtproto/") {
            Some(rest)
        } else {
            return Err(CliError::new(
                "snapshot.unsupported_provider",
                format!(
                    "unsupported snapshot provider: snapshot_id={snapshot_id} provider={snapshot_provider}. TelevyBackup is MTProto-only now. Fix: run a new backup with MTProto."
                ),
            ));
        };

        let ep = select_endpoint(&settings, endpoint_id)?;

        if settings.telegram.mtproto.api_id <= 0 {
            return Err(CliError::new(
                "config.invalid",
                "telegram.mtproto.api_id must be > 0",
            ));
        }
        if settings.telegram.mtproto.api_hash_key.is_empty() {
            return Err(CliError::new(
                "config.invalid",
                "telegram.mtproto.api_hash_key must not be empty",
            ));
        }
        if ep.chat_id.is_empty() {
            return Err(CliError::new(
                "config.invalid",
                format!("telegram_endpoints[{id}].chat_id is empty", id = ep.id),
            ));
        }

        let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
            .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
        let master_key = load_master_key(config_dir, data_dir)?;

        let cache_db = data_dir
            .join("cache")
            .join("remote-index")
            .join(format!("{snapshot_id}.sqlite"));
        if let Some(parent) = cache_db.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
        }

        if events {
            emit_event_stdout(serde_json::json!({
                "type": "task.state",
                "taskId": task_id,
                "kind": "restore",
                "state": "running",
                "snapshotId": snapshot_id,
            }));
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
            throttle: Mutex::new(ProgressThrottle::new(Duration::from_millis(200))),
        };
        let opts = RestoreOptions {
            cancel: None,
            progress: if events { Some(&sink) } else { None },
        };

        let cfg = RestoreConfig {
            snapshot_id: snapshot_id.clone(),
            manifest_object_id,
            master_key,
            index_db_path: cache_db,
            target_path: target,
        };

        let api_hash = get_secret(config_dir, data_dir, &settings.telegram.mtproto.api_hash_key)?.ok_or_else(
            || CliError::new("telegram.mtproto.missing_api_hash", "mtproto api_hash missing"),
        )?;
        let session = load_optional_base64_secret_bytes(
            config_dir,
            data_dir,
            &ep.mtproto.session_key,
            "telegram.mtproto.session_invalid",
            "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
        )?;

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            provider: snapshot_provider.clone(),
            api_id: settings.telegram.mtproto.api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: ep.chat_id.clone(),
            session,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(map_core_err)?;

        let res = restore_snapshot_with(&storage, cfg, opts).await.map_err(map_core_err)?;

        if let Some(bytes) = storage.session_bytes() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            if let Err(e) = set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64) {
                tracing::warn!(
                    event = "secrets.session_persist_failed",
                    error_code = e.code,
                    error_message = %e.message,
                    "failed to persist mtproto session"
                );
            }
        }

        Ok(res)
    }
    .await;

    let duration_seconds = started.elapsed().as_secs_f64();
    match result {
        Ok(res) => {
            tracing::warn!(
                event = "run.finish",
                kind = "restore",
                run_id = %task_id,
                task_id = %task_id,
                snapshot_id = %snapshot_id,
                status = "succeeded",
                duration_seconds,
                files_restored = res.files_restored,
                chunks_downloaded = res.chunks_downloaded,
                bytes_written = res.bytes_written,
                "run.finish"
            );

            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "restore",
                    "state": "succeeded",
                    "snapshotId": snapshot_id,
                    "result": {
                        "filesRestored": res.files_restored,
                        "chunksDownloaded": res.chunks_downloaded,
                        "bytesWritten": res.bytes_written,
                        "durationSeconds": duration_seconds,
                    }
                }));
                return Ok(());
            }

            if json {
                println!("{}", serde_json::json!({ "ok": true }));
            } else {
                println!("ok");
            }
            Ok(())
        }
        Err(e) => {
            tracing::error!(
                event = "run.finish",
                kind = "restore",
                run_id = %task_id,
                task_id = %task_id,
                snapshot_id = %snapshot_id,
                status = "failed",
                duration_seconds,
                error_code = e.code,
                error_message = %e.message,
                retryable = e.retryable,
                "run.finish"
            );
            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "restore",
                    "state": "failed",
                    "snapshotId": snapshot_id,
                    "error": { "code": e.code, "message": e.message.clone() },
                }));
            }
            Err(e)
        }
    }
}

async fn restore_list_latest(
    config_dir: &Path,
    data_dir: &Path,
    endpoint_id: Option<String>,
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let ep = select_endpoint(&settings, endpoint_id.as_deref())?;

    if settings.telegram.mtproto.api_id <= 0 {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mtproto.api_id must be > 0",
        ));
    }
    if settings.telegram.mtproto.api_hash_key.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mtproto.api_hash_key must not be empty",
        ));
    }
    if ep.chat_id.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            format!("telegram_endpoints[{id}].chat_id is empty", id = ep.id),
        ));
    }
    if is_likely_private_chat_id(&ep.chat_id) {
        return Err(CliError::new(
            "bootstrap.unsupported_chat",
            "bootstrap catalog requires message pinning; private chats are not supported. Use a group/channel (e.g. -100...) or an @username chat id.".to_string(),
        ));
    }

    let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let master_key = load_master_key(config_dir, data_dir)?;
    let api_hash = get_secret(
        config_dir,
        data_dir,
        &settings.telegram.mtproto.api_hash_key,
    )?
    .ok_or_else(|| {
        CliError::new(
            "telegram.mtproto.missing_api_hash",
            "mtproto api_hash missing",
        )
    })?;
    let session = load_optional_base64_secret_bytes(
        config_dir,
        data_dir,
        &ep.mtproto.session_key,
        "telegram.mtproto.session_invalid",
        "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
    )?;

    let cache_dir = data_dir.join("cache").join("mtproto");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

    let provider = settings_config::endpoint_provider(&ep.id);
    let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
        provider,
        api_id: settings.telegram.mtproto.api_id,
        api_hash: api_hash.clone(),
        bot_token: bot_token.clone(),
        chat_id: ep.chat_id.clone(),
        session,
        cache_dir,
        helper_path: None,
    })
    .await
    .map_err(map_core_err)?;

    let cat = televy_backup_core::bootstrap::load_remote_catalog(&storage, &master_key)
        .await
        .map_err(map_core_err)?;
    let Some(cat) = cat else {
        return Err(CliError::new(
            "bootstrap.missing",
            "bootstrap missing (no pinned catalog)",
        ));
    };

    if let Some(bytes) = storage.session_bytes() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
        if let Err(e) = set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64) {
            tracing::warn!(
                event = "secrets.session_persist_failed",
                error_code = e.code,
                error_message = %e.message,
                "failed to persist mtproto session"
            );
        }
    }

    if json {
        println!("{}", serde_json::json!({ "catalog": cat }));
        return Ok(());
    }

    println!("updatedAt={}", cat.updated_at);
    for t in cat.targets {
        if let Some(latest) = t.latest {
            println!(
                "targetId={} sourcePath={} snapshotId={} manifestObjectId={}",
                t.target_id, t.source_path, latest.snapshot_id, latest.manifest_object_id
            );
        } else {
            println!(
                "targetId={} sourcePath={} latest=none",
                t.target_id, t.source_path
            );
        }
    }
    Ok(())
}

async fn restore_latest(
    config_dir: &Path,
    data_dir: &Path,
    target_id: Option<String>,
    source_path: Option<PathBuf>,
    target: PathBuf,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
    let run_log = televy_backup_core::run_log::start_run_log("restore", &task_id, data_dir)
        .map_err(|e| CliError::new("log.init_failed", e.to_string()))?;
    let started = std::time::Instant::now();

    let settings = match load_settings(config_dir) {
        Ok(s) => s,
        Err(e) => {
            return emit_preflight_failed(
                events,
                &task_id,
                "restore",
                run_log.path(),
                started,
                RunCtx {
                    target_id: None,
                    endpoint_id: None,
                    source_path: None,
                    snapshot_id: Some("latest"),
                },
                e,
            );
        }
    };

    let t = match select_target(&settings, target_id.as_deref(), source_path.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            return emit_preflight_failed(
                events,
                &task_id,
                "restore",
                run_log.path(),
                started,
                RunCtx {
                    target_id: None,
                    endpoint_id: None,
                    source_path: None,
                    snapshot_id: Some("latest"),
                },
                e,
            );
        }
    };

    let ep = match settings
        .telegram_endpoints
        .iter()
        .find(|e| e.id == t.endpoint_id)
    {
        Some(ep) => ep,
        None => {
            let e = CliError::new(
                "config.invalid",
                format!(
                    "target references unknown endpoint_id: target_id={} endpoint_id={}",
                    t.id, t.endpoint_id
                ),
            );
            return emit_preflight_failed(
                events,
                &task_id,
                "restore",
                run_log.path(),
                started,
                RunCtx {
                    target_id: Some(t.id.as_str()),
                    endpoint_id: None,
                    source_path: Some(t.source_path.as_str()),
                    snapshot_id: Some("latest"),
                },
                e,
            );
        }
    };

    if settings.telegram.mtproto.api_id <= 0 {
        let e = CliError::new("config.invalid", "telegram.mtproto.api_id must be > 0");
        return emit_preflight_failed(
            events,
            &task_id,
            "restore",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(t.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(t.source_path.as_str()),
                snapshot_id: Some("latest"),
            },
            e,
        );
    }
    if settings.telegram.mtproto.api_hash_key.is_empty() {
        let e = CliError::new(
            "config.invalid",
            "telegram.mtproto.api_hash_key must not be empty",
        );
        return emit_preflight_failed(
            events,
            &task_id,
            "restore",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(t.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(t.source_path.as_str()),
                snapshot_id: Some("latest"),
            },
            e,
        );
    }
    if ep.chat_id.is_empty() {
        let e = CliError::new(
            "config.invalid",
            format!("telegram_endpoints[{id}].chat_id is empty", id = ep.id),
        );
        return emit_preflight_failed(
            events,
            &task_id,
            "restore",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(t.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(t.source_path.as_str()),
                snapshot_id: Some("latest"),
            },
            e,
        );
    }
    if is_likely_private_chat_id(&ep.chat_id) {
        let e = CliError::new(
            "bootstrap.unsupported_chat",
            "restore latest requires the pinned bootstrap catalog; private chats are not supported. Use a group/channel (e.g. -100...) or an @username chat id.".to_string(),
        );
        return emit_preflight_failed(
            events,
            &task_id,
            "restore",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(t.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(t.source_path.as_str()),
                snapshot_id: Some("latest"),
            },
            e,
        );
    }

    tracing::warn!(
        event = "run.start",
        kind = "restore",
        run_id = %task_id,
        task_id = %task_id,
        target_id = %t.id,
        endpoint_id = %ep.id,
        source_path = %t.source_path,
        snapshot_id = "latest",
        log_path = %run_log.path().display(),
        "run.start"
    );

    emit_task_state_running(
        events,
        &task_id,
        "restore",
        Some(t.id.as_str()),
        Some("latest"),
    );
    emit_task_progress_preflight(events, &task_id);

    let result: Result<(String, televy_backup_core::RestoreResult), CliError> = async {
        let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
            .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
        let master_key = load_master_key(config_dir, data_dir)?;
        let api_hash = get_secret(
            config_dir,
            data_dir,
            &settings.telegram.mtproto.api_hash_key,
        )?
        .ok_or_else(|| {
            CliError::new(
                "telegram.mtproto.missing_api_hash",
                "mtproto api_hash missing",
            )
        })?;
        let session = load_optional_base64_secret_bytes(
            config_dir,
            data_dir,
            &ep.mtproto.session_key,
            "telegram.mtproto.session_invalid",
            "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
        )?;

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let provider = settings_config::endpoint_provider(&ep.id);
        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            provider: provider.clone(),
            api_id: settings.telegram.mtproto.api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: ep.chat_id.clone(),
            session,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(map_core_err)?;

        let latest = televy_backup_core::bootstrap::resolve_remote_latest(
            &storage,
            &master_key,
            Some(&t.id),
            None,
        )
        .await
        .map_err(map_core_err)?;

        emit_task_state_running(
            events,
            &task_id,
            "restore",
            Some(t.id.as_str()),
            Some(latest.snapshot_id.as_str()),
        );

        let cache_db = data_dir
            .join("cache")
            .join("remote-index")
            .join(format!("{}.sqlite", latest.snapshot_id));
        if let Some(parent) = cache_db.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
            throttle: Mutex::new(ProgressThrottle::new(Duration::from_millis(200))),
        };
        let opts = RestoreOptions {
            cancel: None,
            progress: if events { Some(&sink) } else { None },
        };

        let cfg = RestoreConfig {
            snapshot_id: latest.snapshot_id.clone(),
            manifest_object_id: latest.manifest_object_id,
            master_key,
            index_db_path: cache_db,
            target_path: target,
        };

        let res = restore_snapshot_with(&storage, cfg, opts)
            .await
            .map_err(map_core_err)?;

        if let Some(bytes) = storage.session_bytes() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            if let Err(e) = set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64) {
                tracing::warn!(
                    event = "secrets.session_persist_failed",
                    error_code = e.code,
                    error_message = %e.message,
                    "failed to persist mtproto session"
                );
            }
        }

        Ok((latest.snapshot_id, res))
    }
    .await;

    let duration_seconds = started.elapsed().as_secs_f64();
    match result {
        Ok((snapshot_id, res)) => {
            tracing::warn!(
                event = "run.finish",
                kind = "restore",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %t.id,
                endpoint_id = %ep.id,
                source_path = %t.source_path,
                snapshot_id = %snapshot_id,
                status = "succeeded",
                duration_seconds,
                files_restored = res.files_restored,
                chunks_downloaded = res.chunks_downloaded,
                bytes_written = res.bytes_written,
                "run.finish"
            );

            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "restore",
                    "state": "succeeded",
                    "snapshotId": snapshot_id,
                    "targetId": t.id,
                    "result": {
                        "filesRestored": res.files_restored,
                        "chunksDownloaded": res.chunks_downloaded,
                        "bytesWritten": res.bytes_written,
                        "durationSeconds": duration_seconds,
                    }
                }));
                return Ok(());
            }

            if json {
                println!(
                    "{}",
                    serde_json::json!({ "ok": true, "snapshotId": snapshot_id })
                );
            } else {
                println!("ok");
                println!("snapshotId={snapshot_id}");
            }
            Ok(())
        }
        Err(e) => {
            tracing::error!(
                event = "run.finish",
                kind = "restore",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %t.id,
                endpoint_id = %ep.id,
                source_path = %t.source_path,
                snapshot_id = "latest",
                status = "failed",
                duration_seconds,
                error_code = e.code,
                error_message = %e.message,
                retryable = e.retryable,
                "run.finish"
            );
            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "restore",
                    "state": "failed",
                    "targetId": t.id,
                    "error": { "code": e.code, "message": e.message.clone() },
                }));
            }
            Err(e)
        }
    }
}

async fn verify_latest(
    config_dir: &Path,
    data_dir: &Path,
    target_id: Option<String>,
    source_path: Option<PathBuf>,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
    let run_log = televy_backup_core::run_log::start_run_log("verify", &task_id, data_dir)
        .map_err(|e| CliError::new("log.init_failed", e.to_string()))?;
    let started = std::time::Instant::now();

    let settings = match load_settings(config_dir) {
        Ok(s) => s,
        Err(e) => {
            return emit_preflight_failed(
                events,
                &task_id,
                "verify",
                run_log.path(),
                started,
                RunCtx {
                    target_id: None,
                    endpoint_id: None,
                    source_path: None,
                    snapshot_id: Some("latest"),
                },
                e,
            );
        }
    };

    let t = match select_target(&settings, target_id.as_deref(), source_path.as_deref()) {
        Ok(t) => t,
        Err(e) => {
            return emit_preflight_failed(
                events,
                &task_id,
                "verify",
                run_log.path(),
                started,
                RunCtx {
                    target_id: None,
                    endpoint_id: None,
                    source_path: None,
                    snapshot_id: Some("latest"),
                },
                e,
            );
        }
    };

    let ep = match settings
        .telegram_endpoints
        .iter()
        .find(|e| e.id == t.endpoint_id)
    {
        Some(ep) => ep,
        None => {
            let e = CliError::new(
                "config.invalid",
                format!(
                    "target references unknown endpoint_id: target_id={} endpoint_id={}",
                    t.id, t.endpoint_id
                ),
            );
            return emit_preflight_failed(
                events,
                &task_id,
                "verify",
                run_log.path(),
                started,
                RunCtx {
                    target_id: Some(t.id.as_str()),
                    endpoint_id: None,
                    source_path: Some(t.source_path.as_str()),
                    snapshot_id: Some("latest"),
                },
                e,
            );
        }
    };

    if settings.telegram.mtproto.api_id <= 0 {
        let e = CliError::new("config.invalid", "telegram.mtproto.api_id must be > 0");
        return emit_preflight_failed(
            events,
            &task_id,
            "verify",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(t.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(t.source_path.as_str()),
                snapshot_id: Some("latest"),
            },
            e,
        );
    }
    if settings.telegram.mtproto.api_hash_key.is_empty() {
        let e = CliError::new(
            "config.invalid",
            "telegram.mtproto.api_hash_key must not be empty",
        );
        return emit_preflight_failed(
            events,
            &task_id,
            "verify",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(t.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(t.source_path.as_str()),
                snapshot_id: Some("latest"),
            },
            e,
        );
    }
    if ep.chat_id.is_empty() {
        let e = CliError::new(
            "config.invalid",
            format!("telegram_endpoints[{id}].chat_id is empty", id = ep.id),
        );
        return emit_preflight_failed(
            events,
            &task_id,
            "verify",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(t.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(t.source_path.as_str()),
                snapshot_id: Some("latest"),
            },
            e,
        );
    }
    if is_likely_private_chat_id(&ep.chat_id) {
        let e = CliError::new(
            "bootstrap.unsupported_chat",
            "verify latest requires the pinned bootstrap catalog; private chats are not supported. Use a group/channel (e.g. -100...) or an @username chat id.".to_string(),
        );
        return emit_preflight_failed(
            events,
            &task_id,
            "verify",
            run_log.path(),
            started,
            RunCtx {
                target_id: Some(t.id.as_str()),
                endpoint_id: Some(ep.id.as_str()),
                source_path: Some(t.source_path.as_str()),
                snapshot_id: Some("latest"),
            },
            e,
        );
    }

    tracing::warn!(
        event = "run.start",
        kind = "verify",
        run_id = %task_id,
        task_id = %task_id,
        target_id = %t.id,
        endpoint_id = %ep.id,
        source_path = %t.source_path,
        snapshot_id = "latest",
        log_path = %run_log.path().display(),
        "run.start"
    );

    emit_task_state_running(
        events,
        &task_id,
        "verify",
        Some(t.id.as_str()),
        Some("latest"),
    );
    emit_task_progress_preflight(events, &task_id);

    let result: Result<(String, televy_backup_core::VerifyResult), CliError> = async {
        let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
            .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
        let master_key = load_master_key(config_dir, data_dir)?;
        let api_hash = get_secret(
            config_dir,
            data_dir,
            &settings.telegram.mtproto.api_hash_key,
        )?
        .ok_or_else(|| {
            CliError::new(
                "telegram.mtproto.missing_api_hash",
                "mtproto api_hash missing",
            )
        })?;
        let session = load_optional_base64_secret_bytes(
            config_dir,
            data_dir,
            &ep.mtproto.session_key,
            "telegram.mtproto.session_invalid",
            "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
        )?;

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let provider = settings_config::endpoint_provider(&ep.id);
        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            provider: provider.clone(),
            api_id: settings.telegram.mtproto.api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: ep.chat_id.clone(),
            session,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(map_core_err)?;

        let latest = televy_backup_core::bootstrap::resolve_remote_latest(
            &storage,
            &master_key,
            Some(&t.id),
            None,
        )
        .await
        .map_err(map_core_err)?;

        emit_task_state_running(
            events,
            &task_id,
            "verify",
            Some(t.id.as_str()),
            Some(latest.snapshot_id.as_str()),
        );

        let cache_db = data_dir
            .join("cache")
            .join("remote-index")
            .join(format!("{}.sqlite", latest.snapshot_id));
        if let Some(parent) = cache_db.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
            throttle: Mutex::new(ProgressThrottle::new(Duration::from_millis(200))),
        };
        let opts = VerifyOptions {
            cancel: None,
            progress: if events { Some(&sink) } else { None },
        };

        let cfg = VerifyConfig {
            snapshot_id: latest.snapshot_id.clone(),
            manifest_object_id: latest.manifest_object_id,
            master_key,
            index_db_path: cache_db,
        };

        let res = verify_snapshot_with(&storage, cfg, opts)
            .await
            .map_err(map_core_err)?;

        if let Some(bytes) = storage.session_bytes() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            if let Err(e) = set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64) {
                tracing::warn!(
                    event = "secrets.session_persist_failed",
                    error_code = e.code,
                    error_message = %e.message,
                    "failed to persist mtproto session"
                );
            }
        }

        Ok((latest.snapshot_id, res))
    }
    .await;

    let duration_seconds = started.elapsed().as_secs_f64();
    match result {
        Ok((snapshot_id, res)) => {
            tracing::warn!(
                event = "run.finish",
                kind = "verify",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %t.id,
                endpoint_id = %ep.id,
                source_path = %t.source_path,
                snapshot_id = %snapshot_id,
                status = "succeeded",
                duration_seconds,
                chunks_checked = res.chunks_checked,
                bytes_checked = res.bytes_checked,
                "run.finish"
            );

            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "verify",
                    "state": "succeeded",
                    "snapshotId": snapshot_id,
                    "targetId": t.id,
                    "result": {
                        "chunksChecked": res.chunks_checked,
                        "bytesChecked": res.bytes_checked,
                        "durationSeconds": duration_seconds,
                    }
                }));
                return Ok(());
            }

            if json {
                println!(
                    "{}",
                    serde_json::json!({ "ok": true, "snapshotId": snapshot_id })
                );
            } else {
                println!("ok");
                println!("snapshotId={snapshot_id}");
            }
            Ok(())
        }
        Err(e) => {
            tracing::error!(
                event = "run.finish",
                kind = "verify",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %t.id,
                endpoint_id = %ep.id,
                source_path = %t.source_path,
                snapshot_id = "latest",
                status = "failed",
                duration_seconds,
                error_code = e.code,
                error_message = %e.message,
                retryable = e.retryable,
                "run.finish"
            );
            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "verify",
                    "state": "failed",
                    "targetId": t.id,
                    "error": { "code": e.code, "message": e.message.clone() },
                }));
            }
            Err(e)
        }
    }
}

async fn verify_run(
    config_dir: &Path,
    data_dir: &Path,
    snapshot_id: String,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
    let run_log = televy_backup_core::run_log::start_run_log("verify", &task_id, data_dir)
        .map_err(|e| CliError::new("log.init_failed", e.to_string()))?;

    tracing::warn!(
        event = "run.start",
        kind = "verify",
        run_id = %task_id,
        task_id = %task_id,
        snapshot_id = %snapshot_id,
        log_path = %run_log.path().display(),
        "run.start"
    );

    let started = std::time::Instant::now();
    let result: Result<televy_backup_core::VerifyResult, CliError> = async {
        let settings = load_settings(config_dir)?;

        let (manifest_object_id, snapshot_provider) =
            lookup_manifest_meta_any(data_dir, &snapshot_id).await?;

        let endpoint_id = if snapshot_provider == "telegram.mtproto" {
            None
        } else if let Some(rest) = snapshot_provider.strip_prefix("telegram.mtproto/") {
            Some(rest)
        } else {
            return Err(CliError::new(
                "snapshot.unsupported_provider",
                format!(
                    "unsupported snapshot provider: snapshot_id={snapshot_id} provider={snapshot_provider}. TelevyBackup is MTProto-only now. Fix: run a new backup with MTProto."
                ),
            ));
        };

        let ep = select_endpoint(&settings, endpoint_id)?;

        if settings.telegram.mtproto.api_id <= 0 {
            return Err(CliError::new(
                "config.invalid",
                "telegram.mtproto.api_id must be > 0",
            ));
        }
        if settings.telegram.mtproto.api_hash_key.is_empty() {
            return Err(CliError::new(
                "config.invalid",
                "telegram.mtproto.api_hash_key must not be empty",
            ));
        }
        if ep.chat_id.is_empty() {
            return Err(CliError::new(
                "config.invalid",
                format!("telegram_endpoints[{id}].chat_id is empty", id = ep.id),
            ));
        }

        let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
            .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
        let master_key = load_master_key(config_dir, data_dir)?;

        let cache_db = data_dir
            .join("cache")
            .join("remote-index")
            .join(format!("{snapshot_id}.sqlite"));
        if let Some(parent) = cache_db.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
        }

        if events {
            emit_event_stdout(serde_json::json!({
                "type": "task.state",
                "taskId": task_id,
                "kind": "verify",
                "state": "running",
                "snapshotId": snapshot_id,
            }));
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
            throttle: Mutex::new(ProgressThrottle::new(Duration::from_millis(200))),
        };
        let opts = VerifyOptions {
            cancel: None,
            progress: if events { Some(&sink) } else { None },
        };

        let cfg = VerifyConfig {
            snapshot_id: snapshot_id.clone(),
            manifest_object_id,
            master_key,
            index_db_path: cache_db,
        };

        let api_hash = get_secret(config_dir, data_dir, &settings.telegram.mtproto.api_hash_key)?.ok_or_else(
            || CliError::new("telegram.mtproto.missing_api_hash", "mtproto api_hash missing"),
        )?;
        let session = load_optional_base64_secret_bytes(
            config_dir,
            data_dir,
            &ep.mtproto.session_key,
            "telegram.mtproto.session_invalid",
            "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
        )?;

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            provider: snapshot_provider.clone(),
            api_id: settings.telegram.mtproto.api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: ep.chat_id.clone(),
            session,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(map_core_err)?;

        let res = verify_snapshot_with(&storage, cfg, opts).await.map_err(map_core_err)?;

        if let Some(bytes) = storage.session_bytes() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            if let Err(e) = set_secret(config_dir, data_dir, &ep.mtproto.session_key, &b64) {
                tracing::warn!(
                    event = "secrets.session_persist_failed",
                    error_code = e.code,
                    error_message = %e.message,
                    "failed to persist mtproto session"
                );
            }
        }

        Ok(res)
    }
    .await;

    let duration_seconds = started.elapsed().as_secs_f64();
    match result {
        Ok(res) => {
            tracing::warn!(
                event = "run.finish",
                kind = "verify",
                run_id = %task_id,
                task_id = %task_id,
                snapshot_id = %snapshot_id,
                status = "succeeded",
                duration_seconds,
                chunks_checked = res.chunks_checked,
                bytes_checked = res.bytes_checked,
                "run.finish"
            );

            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "verify",
                    "state": "succeeded",
                    "snapshotId": snapshot_id,
                    "result": {
                        "chunksChecked": res.chunks_checked,
                        "bytesChecked": res.bytes_checked,
                        "durationSeconds": duration_seconds,
                    }
                }));
                return Ok(());
            }

            if json {
                println!("{}", serde_json::json!({ "ok": true }));
            } else {
                println!("ok");
            }
            Ok(())
        }
        Err(e) => {
            tracing::error!(
                event = "run.finish",
                kind = "verify",
                run_id = %task_id,
                task_id = %task_id,
                snapshot_id = %snapshot_id,
                status = "failed",
                duration_seconds,
                error_code = e.code,
                error_message = %e.message,
                retryable = e.retryable,
                "run.finish"
            );
            if events {
                emit_event_stdout(serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "verify",
                    "state": "failed",
                    "snapshotId": snapshot_id,
                    "error": { "code": e.code, "message": e.message.clone() },
                }));
            }
            Err(e)
        }
    }
}

async fn lookup_manifest_meta(
    db_path: &Path,
    snapshot_id: &str,
) -> Result<(String, String), CliError> {
    let pool = televy_backup_core::index_db::open_existing_index_db(db_path)
        .await
        .map_err(map_core_err)?;

    let row = sqlx::query(
        "SELECT manifest_object_id, provider FROM remote_indexes WHERE snapshot_id = ? LIMIT 1",
    )
    .bind(snapshot_id)
    .fetch_optional(&pool)
    .await
    .map_err(|e| CliError::new("db.failed", e.to_string()))?;

    match row {
        Some(r) => Ok((
            r.get::<String, _>("manifest_object_id"),
            r.get::<String, _>("provider"),
        )),
        None => Err(CliError::new(
            "snapshot.not_found",
            "manifest not found in local db",
        )),
    }
}

async fn lookup_manifest_meta_any(
    data_dir: &Path,
    snapshot_id: &str,
) -> Result<(String, String), CliError> {
    let global = legacy_global_index_db_path(data_dir);
    if global.exists()
        && let Ok(found) = lookup_manifest_meta(&global, snapshot_id).await
    {
        return Ok(found);
    }

    let index_dir = data_dir.join("index");
    let entries = std::fs::read_dir(&index_dir)
        .map_err(|e| CliError::new("snapshot.not_found", e.to_string()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };
        if name == "index.sqlite" {
            continue;
        }
        if !name.starts_with("index.") || !name.ends_with(".sqlite") {
            continue;
        }

        match lookup_manifest_meta(&path, snapshot_id).await {
            Ok(found) => return Ok(found),
            Err(e) if e.code == "snapshot.not_found" => continue,
            Err(_) => continue,
        }
    }

    Err(CliError::new(
        "snapshot.not_found",
        "manifest not found in local db",
    ))
}

const MASTER_KEY_KEY: &str = "televybackup.master_key";

fn default_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join(APP_NAME)
}

fn default_data_dir() -> PathBuf {
    default_config_dir()
}

fn load_settings(config_dir: &Path) -> Result<Settings, CliError> {
    let settings = settings_config::load_settings_v2(config_dir).map_err(map_core_err)?;
    settings_config::validate_settings_schema_v2(&settings).map_err(map_core_err)?;
    Ok(settings)
}

fn redact_secret(s: impl Into<String>, secret: &str) -> String {
    let s = s.into();
    if secret.is_empty() {
        s
    } else {
        s.replace(secret, "[redacted]")
    }
}

#[derive(Debug, serde::Serialize)]
#[serde(tag = "type")]
enum VaultIpcRequest {
    #[serde(rename = "vault.get_or_create")]
    VaultGetOrCreate,

    #[serde(rename = "keychain.get")]
    KeychainGet { key: String },

    #[serde(rename = "keychain.delete")]
    KeychainDelete { key: String },
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VaultIpcResponse {
    ok: bool,
    vault_key_b64: Option<String>,
    value: Option<String>,
    deleted: Option<bool>,
    error: Option<String>,
}

#[cfg(unix)]
fn vault_ipc_call(data_dir: &Path, req: &VaultIpcRequest) -> Result<VaultIpcResponse, CliError> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    let socket_path = televy_backup_core::secrets::vault_ipc_socket_path(data_dir);
    let mut stream = UnixStream::connect(&socket_path).map_err(|e| {
        CliError::retryable("daemon.unavailable", "vault IPC unavailable").with_details(
            serde_json::json!({
                "socketPath": socket_path.display().to_string(),
                "error": e.to_string(),
            }),
        )
    })?;

    // First-time Keychain access may require user interaction, which can take seconds.
    // Keep timeouts generous to avoid flaky UX.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(120)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(120)));

    let line = serde_json::to_string(req)
        .map_err(|e| CliError::new("daemon.unavailable", e.to_string()))?;
    stream
        .write_all(line.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
        .map_err(|e| {
            CliError::retryable("daemon.unavailable", "vault IPC write failed").with_details(
                serde_json::json!({
                    "socketPath": socket_path.display().to_string(),
                    "error": e.to_string(),
                }),
            )
        })?;
    let _ = stream.flush();

    let mut reader = BufReader::new(stream);
    let mut resp_line = String::new();
    reader.read_line(&mut resp_line).map_err(|e| {
        CliError::retryable("daemon.unavailable", "vault IPC read failed").with_details(
            serde_json::json!({
                "socketPath": socket_path.display().to_string(),
                "error": e.to_string(),
            }),
        )
    })?;

    let resp: VaultIpcResponse = serde_json::from_str(resp_line.trim_end())
        .map_err(|e| CliError::new("daemon.unavailable", format!("invalid IPC response: {e}")))?;

    if resp.ok {
        Ok(resp)
    } else {
        Err(CliError::new(
            "daemon.failed",
            resp.error
                .unwrap_or_else(|| "daemon request failed".to_string()),
        ))
    }
}

#[cfg(not(unix))]
fn vault_ipc_call(_data_dir: &Path, _req: &VaultIpcRequest) -> Result<VaultIpcResponse, CliError> {
    Err(CliError::new(
        "daemon.unavailable",
        "vault IPC is only supported on unix",
    ))
}

fn daemon_vault_get_or_create_b64(data_dir: &Path) -> Result<String, CliError> {
    let resp = vault_ipc_call(data_dir, &VaultIpcRequest::VaultGetOrCreate)?;
    resp.vault_key_b64
        .ok_or_else(|| CliError::new("daemon.failed", "vault IPC missing vault_key_b64"))
}

#[cfg(unix)]
fn control_ipc_call_with_timeouts(
    data_dir: &Path,
    method: &str,
    params: serde_json::Value,
    read_timeout: Duration,
    write_timeout: Duration,
) -> Result<televy_backup_core::control::ControlResponse, CliError> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;

    fn is_timeout_io_error(e: &std::io::Error) -> bool {
        matches!(
            e.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        )
    }

    let socket_path = televy_backup_core::control::control_ipc_socket_path(data_dir);
    let mut stream = UnixStream::connect(&socket_path).map_err(|e| {
        CliError::retryable(
            "control.unavailable",
            "control IPC unavailable (is daemon running?)",
        )
        .with_details(serde_json::json!({
            "socketPath": socket_path.display().to_string(),
            "error": e.to_string(),
        }))
    })?;

    let _ = stream.set_read_timeout(Some(read_timeout));
    let _ = stream.set_write_timeout(Some(write_timeout));

    let req = televy_backup_core::control::ControlRequest::new(
        uuid::Uuid::new_v4().to_string(),
        method,
        params,
    );
    let line = serde_json::to_string(&req)
        .map_err(|e| CliError::new("control.unavailable", e.to_string()))?;
    stream
        .write_all(line.as_bytes())
        .and_then(|_| stream.write_all(b"\n"))
        .map_err(|e| {
            if is_timeout_io_error(&e) {
                CliError::retryable("control.timeout", "control IPC timeout").with_details(
                    serde_json::json!({
                        "socketPath": socket_path.display().to_string(),
                        "error": e.to_string(),
                    }),
                )
            } else {
                CliError::retryable("control.unavailable", "control IPC write failed").with_details(
                    serde_json::json!({
                        "socketPath": socket_path.display().to_string(),
                        "error": e.to_string(),
                    }),
                )
            }
        })?;
    let _ = stream.flush();

    let mut reader = BufReader::new(stream);
    let mut resp_line = String::new();
    match reader.read_line(&mut resp_line) {
        Ok(0) => Err(CliError::retryable(
            "control.unavailable",
            "control IPC closed before response",
        )
        .with_details(serde_json::json!({
            "socketPath": socket_path.display().to_string(),
        }))),
        Ok(_) => Ok(()),
        Err(e) => Err(if is_timeout_io_error(&e) {
            CliError::retryable("control.timeout", "control IPC timeout").with_details(
                serde_json::json!({
                    "socketPath": socket_path.display().to_string(),
                    "error": e.to_string(),
                }),
            )
        } else {
            CliError::retryable("control.unavailable", "control IPC read failed").with_details(
                serde_json::json!({
                    "socketPath": socket_path.display().to_string(),
                    "error": e.to_string(),
                }),
            )
        }),
    }?;

    let resp: televy_backup_core::control::ControlResponse =
        serde_json::from_str(resp_line.trim_end()).map_err(|e| {
            CliError::new("control.unavailable", format!("invalid IPC response: {e}")).with_details(
                serde_json::json!({
                    "socketPath": socket_path.display().to_string(),
                    "responseLine": resp_line.clone(),
                }),
            )
        })?;

    if resp.ok {
        Ok(resp)
    } else {
        let err = resp
            .error
            .unwrap_or(televy_backup_core::control::ControlError {
                code: "control.failed".to_string(),
                message: "daemon request failed".to_string(),
                retryable: false,
                details: serde_json::json!({}),
            });

        let (code, details) = match err.code.as_str() {
            "control.unavailable" => ("control.unavailable", err.details),
            "control.timeout" => ("control.timeout", err.details),
            "control.invalid_request" => ("control.invalid_request", err.details),
            "control.method_not_found" => ("control.method_not_found", err.details),
            _ => (
                "control.failed",
                serde_json::json!({
                    "daemonCode": err.code,
                    "daemonDetails": err.details,
                }),
            ),
        };

        let out = if err.retryable {
            CliError::retryable(code, err.message)
        } else {
            CliError::new(code, err.message)
        };
        Err(out.with_details(details))
    }
}

#[cfg(unix)]
fn control_ipc_call(
    data_dir: &Path,
    method: &str,
    params: serde_json::Value,
) -> Result<televy_backup_core::control::ControlResponse, CliError> {
    control_ipc_call_with_timeouts(
        data_dir,
        method,
        params,
        Duration::from_secs(30),
        Duration::from_secs(5),
    )
}

#[cfg(not(unix))]
fn control_ipc_call(
    _data_dir: &Path,
    _method: &str,
    _params: serde_json::Value,
) -> Result<televy_backup_core::control::ControlResponse, CliError> {
    Err(CliError::new(
        "daemon.unavailable",
        "control IPC is only supported on unix",
    ))
}

#[cfg(all(test, unix))]
mod control_ipc_tests {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::thread;

    use super::*;

    fn wait_for_socket(path: &Path) {
        for _ in 0..50 {
            if path.exists() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("socket did not appear: {}", path.display());
    }

    #[test]
    fn control_ipc_missing_socket_maps_to_control_unavailable() {
        let dir = tempfile::tempdir().unwrap();

        let err = control_ipc_call_with_timeouts(
            dir.path(),
            "secrets.presence",
            serde_json::json!({}),
            Duration::from_millis(20),
            Duration::from_millis(20),
        )
        .unwrap_err();

        assert_eq!(err.code, "control.unavailable");
        assert!(err.retryable);
    }

    #[test]
    fn control_ipc_timeout_maps_to_control_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let ipc_dir = dir.path().join("ipc");
        std::fs::create_dir_all(&ipc_dir).unwrap();
        let socket_path = ipc_dir.join("control.sock");

        let server = thread::spawn({
            let socket_path = socket_path.clone();
            move || {
                let listener = UnixListener::bind(socket_path).unwrap();
                let (stream, _addr) = listener.accept().unwrap();
                let mut r = BufReader::new(stream.try_clone().unwrap());
                let mut _line = String::new();
                let _ = r.read_line(&mut _line);
                thread::sleep(Duration::from_millis(100));
            }
        });

        wait_for_socket(&socket_path);
        let err = control_ipc_call_with_timeouts(
            dir.path(),
            "secrets.presence",
            serde_json::json!({}),
            Duration::from_millis(20),
            Duration::from_millis(20),
        )
        .unwrap_err();

        assert_eq!(err.code, "control.timeout");
        assert!(err.retryable);

        server.join().unwrap();
    }

    #[test]
    fn control_ipc_method_not_found_maps_code() {
        let dir = tempfile::tempdir().unwrap();
        let ipc_dir = dir.path().join("ipc");
        std::fs::create_dir_all(&ipc_dir).unwrap();
        let socket_path = ipc_dir.join("control.sock");

        let server = thread::spawn({
            let socket_path = socket_path.clone();
            move || {
                let listener = UnixListener::bind(socket_path).unwrap();
                let (mut stream, _addr) = listener.accept().unwrap();

                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();

                let req: televy_backup_core::control::ControlRequest =
                    serde_json::from_str(line.trim_end()).unwrap();
                let resp = televy_backup_core::control::ControlResponse::err(
                    req.id,
                    televy_backup_core::control::ControlError::method_not_found(
                        "unknown method",
                        serde_json::json!({}),
                    ),
                );
                let resp_line = serde_json::to_string(&resp).unwrap() + "\n";
                stream.write_all(resp_line.as_bytes()).unwrap();
                let _ = stream.flush();
            }
        });

        wait_for_socket(&socket_path);
        let err = control_ipc_call_with_timeouts(
            dir.path(),
            "unknown.method",
            serde_json::json!({}),
            Duration::from_millis(200),
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert_eq!(err.code, "control.method_not_found");
        assert!(!err.retryable);

        server.join().unwrap();
    }

    #[test]
    fn control_ipc_preserves_daemon_error_code_in_details() {
        let dir = tempfile::tempdir().unwrap();
        let ipc_dir = dir.path().join("ipc");
        std::fs::create_dir_all(&ipc_dir).unwrap();
        let socket_path = ipc_dir.join("control.sock");

        let server = thread::spawn({
            let socket_path = socket_path.clone();
            move || {
                let listener = UnixListener::bind(socket_path).unwrap();
                let (mut stream, _addr) = listener.accept().unwrap();

                let mut line = String::new();
                BufReader::new(stream.try_clone().unwrap())
                    .read_line(&mut line)
                    .unwrap();

                let req: televy_backup_core::control::ControlRequest =
                    serde_json::from_str(line.trim_end()).unwrap();
                let resp = televy_backup_core::control::ControlResponse::err(
                    req.id,
                    televy_backup_core::control::ControlError {
                        code: "secrets.vault_unavailable".to_string(),
                        message: "vault unavailable".to_string(),
                        retryable: false,
                        details: serde_json::json!({ "hint": "test" }),
                    },
                );
                let resp_line = serde_json::to_string(&resp).unwrap() + "\n";
                stream.write_all(resp_line.as_bytes()).unwrap();
                let _ = stream.flush();
            }
        });

        wait_for_socket(&socket_path);
        let err = control_ipc_call_with_timeouts(
            dir.path(),
            "secrets.presence",
            serde_json::json!({}),
            Duration::from_millis(200),
            Duration::from_millis(200),
        )
        .unwrap_err();

        assert_eq!(err.code, "control.failed");
        assert_eq!(
            err.details.get("daemonCode").and_then(|v| v.as_str()),
            Some("secrets.vault_unavailable")
        );

        server.join().unwrap();
    }
}

fn daemon_control_secrets_presence(
    data_dir: &Path,
    endpoint_id: Option<&str>,
) -> Result<serde_json::Value, CliError> {
    let params = serde_json::json!({ "endpointId": endpoint_id });
    let resp = control_ipc_call(data_dir, "secrets.presence", params)?;
    resp.result
        .ok_or_else(|| CliError::new("control.failed", "missing result"))
}

fn daemon_control_secrets_set_telegram_bot_token(
    data_dir: &Path,
    endpoint_id: &str,
    token: &str,
) -> Result<(), CliError> {
    let params = serde_json::json!({ "endpointId": endpoint_id, "token": token });
    let _ = control_ipc_call(data_dir, "secrets.setTelegramBotToken", params)?;
    Ok(())
}

fn daemon_control_secrets_set_telegram_api_hash(
    data_dir: &Path,
    api_hash: &str,
) -> Result<(), CliError> {
    let params = serde_json::json!({ "apiHash": api_hash });
    let _ = control_ipc_call(data_dir, "secrets.setTelegramApiHash", params)?;
    Ok(())
}

fn daemon_control_secrets_clear_telegram_mtproto_session(
    data_dir: &Path,
    endpoint_id: &str,
) -> Result<(), CliError> {
    let params = serde_json::json!({ "endpointId": endpoint_id });
    let _ = control_ipc_call(data_dir, "secrets.clearTelegramMtprotoSession", params)?;
    Ok(())
}

fn daemon_keychain_get_secret(data_dir: &Path, key: &str) -> Result<Option<String>, CliError> {
    let resp = vault_ipc_call(
        data_dir,
        &VaultIpcRequest::KeychainGet {
            key: key.to_string(),
        },
    )?;
    Ok(resp.value)
}

fn daemon_keychain_delete_secret(data_dir: &Path, key: &str) -> Result<bool, CliError> {
    let resp = vault_ipc_call(
        data_dir,
        &VaultIpcRequest::KeychainDelete {
            key: key.to_string(),
        },
    )?;
    Ok(resp.deleted.unwrap_or(false))
}

fn load_or_create_vault_key(data_dir: &Path) -> Result<[u8; 32], CliError> {
    if let Some(key) = VAULT_KEY_CACHE.get() {
        return Ok(*key);
    }

    let b64 = daemon_vault_get_or_create_b64(data_dir)?;
    let key = televy_backup_core::secrets::vault_key_from_base64(&b64)
        .map_err(|e| CliError::new("secrets.vault_unavailable", e.to_string()))?;
    let _ = VAULT_KEY_CACHE.set(key);
    Ok(key)
}

fn get_secret(config_dir: &Path, data_dir: &Path, key: &str) -> Result<Option<String>, CliError> {
    let vault_key = load_or_create_vault_key(data_dir)?;
    let path = televy_backup_core::secrets::secrets_path(config_dir);
    let store = televy_backup_core::secrets::load_secrets_store(&path, &vault_key)
        .map_err(map_secrets_store_err)?;
    Ok(store.get(key).map(|s| s.to_string()))
}

fn set_secret(config_dir: &Path, data_dir: &Path, key: &str, value: &str) -> Result<(), CliError> {
    let vault_key = load_or_create_vault_key(data_dir)?;
    let path = televy_backup_core::secrets::secrets_path(config_dir);
    let mut store = televy_backup_core::secrets::load_secrets_store(&path, &vault_key)
        .map_err(map_secrets_store_err)?;
    store.set(key, value);
    televy_backup_core::secrets::save_secrets_store(&path, &vault_key, &store)
        .map_err(map_secrets_store_err)?;
    Ok(())
}

fn map_secrets_store_err(e: televy_backup_core::secrets::SecretsStoreError) -> CliError {
    CliError::new("secrets.store_failed", e.to_string())
}

fn load_master_key(config_dir: &Path, data_dir: &Path) -> Result<[u8; 32], CliError> {
    let b64 = get_secret(config_dir, data_dir, MASTER_KEY_KEY)?
        .ok_or_else(|| CliError::new("config.invalid", "master key missing"))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| CliError::new("config.invalid", e.to_string()))?;
    bytes
        .try_into()
        .map_err(|_| CliError::new("config.invalid", "invalid master key length"))
}

fn map_core_err(e: televy_backup_core::Error) -> CliError {
    match e {
        televy_backup_core::Error::InvalidConfig { message } => {
            CliError::new("config.invalid", message)
        }
        televy_backup_core::Error::BootstrapMissing { message } => {
            CliError::new("bootstrap.missing", message)
        }
        televy_backup_core::Error::BootstrapDecryptFailed { message } => CliError::new(
            "bootstrap.decrypt_failed",
            "pinned bootstrap catalog exists but cannot be decrypted; import the correct master key (TBK1)".to_string(),
        )
        .with_details(serde_json::json!({ "cause": message })),
        televy_backup_core::Error::Crypto { message } => {
            CliError::new("crypto", format!("crypto error: {message}"))
        }
        televy_backup_core::Error::Telegram { message } => {
            CliError::retryable("telegram.unavailable", message)
        }
        televy_backup_core::Error::MissingChunkObject { chunk_hash } => {
            CliError::new("chunk.missing", format!("missing chunk: {chunk_hash}"))
        }
        televy_backup_core::Error::MissingIndexPart {
            snapshot_id,
            part_no,
        } => CliError::new(
            "index.part_missing",
            format!("missing index part: snapshot_id={snapshot_id} part_no={part_no}"),
        ),
        televy_backup_core::Error::Integrity { message } => CliError::new("integrity", message),
        televy_backup_core::Error::Cancelled => CliError::new("task.cancelled", "cancelled"),
        other => CliError::new("unknown", other.to_string()),
    }
}

fn emit_error(e: &CliError) {
    let json = serde_json::to_string(e).unwrap_or_else(|_| "{\"code\":\"unknown\",\"message\":\"json encode failed\",\"details\":{},\"retryable\":false}".to_string());
    let _ = writeln!(std::io::stderr(), "{json}");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_throttle_emits_first_event_phase_changes_and_rate_limits() {
        let mut t = ProgressThrottle::new(Duration::from_millis(50));
        assert!(t.should_emit("scan"));
        assert!(!t.should_emit("scan"), "should throttle within interval");

        // Phase change should bypass the cadence throttle.
        assert!(t.should_emit("upload"));
        assert!(
            !t.should_emit("upload"),
            "should throttle again after phase emit"
        );

        std::thread::sleep(Duration::from_millis(60));
        assert!(t.should_emit("upload"), "should emit again after interval");
    }

    fn endpoint(id: &str) -> settings_config::TelegramEndpoint {
        settings_config::TelegramEndpoint {
            id: id.to_string(),
            mode: "mtproto".to_string(),
            chat_id: "-1001".to_string(),
            bot_token_key: format!("telegram.bot_token.{id}"),
            mtproto: settings_config::TelegramEndpointMtproto::default(),
            rate_limit: settings_config::TelegramRateLimit::default(),
        }
    }

    fn write_config(dir: &Path, text: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(settings_config::config_path(dir), text).unwrap();
    }

    fn temp_config_dir(name: &str) -> PathBuf {
        let mut dir = std::env::temp_dir();
        dir.push(format!(
            "televybackup-cli-test-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        dir
    }

    #[tokio::test]
    async fn init_empty_index_db_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let index_dir = dir.path().join("index");
        let path = index_dir.join("index.ep1.sqlite.tmp");
        assert!(!index_dir.exists());

        init_empty_index_db(&path).await.unwrap();

        assert!(index_dir.exists());
        assert!(path.exists());
    }

    #[test]
    fn load_settings_rejects_duplicate_endpoint_ids() {
        let dir = temp_config_dir("dup-endpoint");
        write_config(
            &dir,
            r#"
version = 2

[[telegram_endpoints]]
id = "ep1"
mode = "mtproto"
chat_id = "-1001"
bot_token_key = "telegram.bot_token.ep1"

[[telegram_endpoints]]
id = "ep1"
mode = "mtproto"
chat_id = "-1002"
bot_token_key = "telegram.bot_token.ep1b"
"#,
        );

        let err = load_settings(&dir).unwrap_err();
        assert_eq!(err.code, "config.invalid");
        assert!(err.message.contains("duplicate telegram_endpoints id"));
    }

    #[test]
    fn load_settings_rejects_target_unknown_endpoint_reference() {
        let dir = temp_config_dir("unknown-endpoint");
        write_config(
            &dir,
            r#"
version = 2

[[telegram_endpoints]]
id = "ep1"
mode = "mtproto"
chat_id = "-1001"
bot_token_key = "telegram.bot_token.ep1"

[[targets]]
id = "t1"
source_path = "/tmp"
endpoint_id = "missing"
"#,
        );

        let err = load_settings(&dir).unwrap_err();
        assert_eq!(err.code, "config.invalid");
        assert!(err.message.contains("references unknown endpoint_id"));
    }

    #[test]
    fn select_endpoint_defaults_to_only_endpoint() {
        let settings = Settings {
            telegram_endpoints: vec![endpoint("ep1")],
            ..Default::default()
        };
        let ep = select_endpoint(&settings, None).unwrap();
        assert_eq!(ep.id, "ep1");
    }

    #[test]
    fn select_endpoint_prefers_default_when_present() {
        let settings = Settings {
            telegram_endpoints: vec![endpoint("ep1"), endpoint("default"), endpoint("ep2")],
            ..Default::default()
        };
        let ep = select_endpoint(&settings, None).unwrap();
        assert_eq!(ep.id, "default");
    }

    #[test]
    fn select_endpoint_errors_when_ambiguous() {
        let settings = Settings {
            telegram_endpoints: vec![endpoint("ep1"), endpoint("ep2")],
            ..Default::default()
        };
        let err = select_endpoint(&settings, None).unwrap_err();
        assert_eq!(err.code, "config.invalid");
        assert!(err.message.contains("multiple endpoints configured"));
    }
}
