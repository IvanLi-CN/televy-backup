use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
#[cfg(target_os = "macos")]
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use base64::Engine;
use clap::{Parser, Subcommand};
use serde::Serialize;
use sqlx::Row;
use televy_backup_core::{
    APP_NAME, BackupConfig, BackupOptions, ChunkingConfig, ProgressSink, RestoreConfig,
    RestoreOptions, Storage, TelegramMtProtoStorage, TelegramMtProtoStorageConfig, VerifyConfig,
    VerifyOptions, restore_snapshot_with, run_backup_with, verify_snapshot_with,
};
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
}

static VAULT_KEY_CACHE: OnceLock<[u8; 32]> = OnceLock::new();

impl ProgressSink for NdjsonProgressSink {
    fn on_progress(&self, p: televy_backup_core::TaskProgress) {
        let line = serde_json::json!({
            "type": "task.progress",
            "taskId": self.task_id,
            "phase": p.phase,
            "filesTotal": p.files_total,
            "filesDone": p.files_done,
            "chunksTotal": p.chunks_total,
            "chunksDone": p.chunks_done,
            "bytesRead": p.bytes_read,
            "bytesUploaded": p.bytes_uploaded,
            "bytesDeduped": p.bytes_deduped,
        });
        println!("{line}");
        let _ = std::io::stdout().flush();
    }
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
            SecretsCmd::MigrateKeychain => secrets_migrate_keychain(&config_dir, &data_dir, cli.json).await,
            SecretsCmd::InitMasterKey => secrets_init_master_key(&config_dir, &data_dir, cli.json).await,
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
            } => {
                backup_run(
                    &config_dir,
                    &data_dir,
                    target_id,
                    source,
                    label,
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
            let master_present = get_secret(config_dir, data_dir, MASTER_KEY_KEY)?.is_some();

            let mtproto_api_hash_present =
                get_secret(config_dir, data_dir, &settings.telegram.mtproto.api_hash_key)?.is_some();

            let mut bot_present_by_endpoint = serde_json::Map::<String, serde_json::Value>::new();
            let mut mtproto_session_present_by_endpoint =
                serde_json::Map::<String, serde_json::Value>::new();
            for ep in &settings.telegram_endpoints {
                let bot_present = get_secret(config_dir, data_dir, &ep.bot_token_key)?.is_some();
                bot_present_by_endpoint.insert(ep.id.clone(), serde_json::Value::Bool(bot_present));

                let sess_present =
                    get_secret(config_dir, data_dir, &ep.mtproto.session_key)?.is_some();
                mtproto_session_present_by_endpoint
                    .insert(ep.id.clone(), serde_json::Value::Bool(sess_present));
            }
            println!(
                "{}",
                serde_json::json!({
                    "settings": settings,
                    "secrets": {
                        "telegramBotTokenPresentByEndpoint": bot_present_by_endpoint,
                        "masterKeyPresent": master_present,
                        "telegramMtprotoApiHashPresent": mtproto_api_hash_present,
                        "telegramMtprotoSessionPresentByEndpoint": mtproto_session_present_by_endpoint
                    }
                })
            );
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
            let master_present = get_secret(config_dir, data_dir, MASTER_KEY_KEY)?.is_some();
            let mtproto_api_hash_present =
                get_secret(config_dir, data_dir, &settings.telegram.mtproto.api_hash_key)?.is_some();
            println!();
            println!("masterKeyPresent={master_present}");
            println!("telegramMtprotoApiHashPresent={mtproto_api_hash_present}");
            for ep in &settings.telegram_endpoints {
                let telegram_present = get_secret(config_dir, data_dir, &ep.bot_token_key)?.is_some();
                let mtproto_session_present =
                    get_secret(config_dir, data_dir, &ep.mtproto.session_key)?.is_some();
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
    set_secret(config_dir, data_dir, &ep.bot_token_key, &token)?;

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

    set_secret(
        config_dir,
        data_dir,
        &settings.telegram.mtproto.api_hash_key,
        &api_hash,
    )?;

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
        delete_secret(config_dir, data_dir, &ep.mtproto.session_key)?;
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
    if ep.chat_id.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            format!("telegram_endpoints[{id}].chat_id is empty", id = ep.id),
        ));
    }

    let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let api_hash =
        get_secret(config_dir, data_dir, &settings.telegram.mtproto.api_hash_key)?.ok_or_else(|| {
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

async fn snapshots_list(data_dir: &Path, limit: u32, json: bool) -> Result<(), CliError> {
    let db_path = data_dir.join("index").join("index.sqlite");
    if !db_path.exists() {
        if json {
            println!("{}", serde_json::json!({ "snapshots": [] }));
        }
        return Ok(());
    }

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

    let mut out = Vec::new();
    for row in rows {
        out.push(serde_json::json!({
            "snapshotId": row.get::<String, _>("snapshot_id"),
            "createdAt": row.get::<String, _>("created_at"),
            "sourcePath": row.get::<String, _>("source_path"),
            "label": row.get::<String, _>("label"),
            "baseSnapshotId": row.get::<Option<String>, _>("base_snapshot_id"),
        }));
    }

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
    let db_path = data_dir.join("index").join("index.sqlite");
    if !db_path.exists() {
        if json {
            println!(
                "{}",
                serde_json::json!({ "snapshotsTotal": 0, "chunksTotal": 0, "chunksBytesTotal": 0 })
            );
        }
        return Ok(());
    }

    let pool = televy_backup_core::index_db::open_existing_index_db(&db_path)
        .await
        .map_err(map_core_err)?;

    let snapshots_total: i64 = sqlx::query("SELECT COUNT(1) as c FROM snapshots")
        .fetch_one(&pool)
        .await
        .map_err(|e| CliError::new("db.failed", e.to_string()))?
        .get("c");
    let chunks_total: i64 = sqlx::query("SELECT COUNT(1) as c FROM chunks")
        .fetch_one(&pool)
        .await
        .map_err(|e| CliError::new("db.failed", e.to_string()))?
        .get("c");
    let chunks_bytes_total: i64 = sqlx::query("SELECT COALESCE(SUM(size), 0) as s FROM chunks")
        .fetch_one(&pool)
        .await
        .map_err(|e| CliError::new("db.failed", e.to_string()))?
        .get("s");

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
    let db_path = data_dir.join("index").join("index.sqlite");
    if !db_path.exists() {
        if json {
            println!(
                "{}",
                serde_json::json!({ "snapshot": serde_json::Value::Null })
            );
        }
        return Ok(());
    }

    let pool = televy_backup_core::index_db::open_existing_index_db(&db_path)
        .await
        .map_err(map_core_err)?;

    let snapshot_row: Option<sqlx::sqlite::SqliteRow> = if let Some(source) = &source {
        let source = source
            .to_str()
            .ok_or_else(|| CliError::new("config.invalid", "source path is not valid utf-8"))?
            .to_string();
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
        if json {
            println!(
                "{}",
                serde_json::json!({ "snapshot": serde_json::Value::Null })
            );
        }
        return Ok(());
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

async fn backup_run(
    config_dir: &Path,
    data_dir: &Path,
    target_id: Option<String>,
    source: Option<PathBuf>,
    label: String,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
    let run_log = televy_backup_core::run_log::start_run_log("backup", &task_id, data_dir)
        .map_err(|e| CliError::new("log.init_failed", e.to_string()))?;

    tracing::info!(
        event = "run.start",
        kind = "backup",
        run_id = %task_id,
        task_id = %task_id,
        log_path = %run_log.path().display(),
        "run.start"
    );

    let started = std::time::Instant::now();
    let result: Result<televy_backup_core::BackupResult, CliError> = async {
        let settings = load_settings(config_dir)?;
        let target = select_target(&settings, target_id.as_deref(), source.as_deref())?;
        let ep = settings
            .telegram_endpoints
            .iter()
            .find(|e| e.id == target.endpoint_id)
            .ok_or_else(|| {
                CliError::new(
                    "config.invalid",
                    format!(
                        "target references unknown endpoint_id: target_id={} endpoint_id={}",
                        target.id, target.endpoint_id
                    ),
                )
            })?;

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

        let db_path = data_dir.join("index").join("index.sqlite");
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
        }

        if events {
            println!(
                "{}",
                serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "backup",
                    "state": "running"
                })
            );
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
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

        let res = run_backup_with(&storage, cfg, opts)
            .await
            .map_err(map_core_err)?;

        // Update remote bootstrap/catalog for cross-device restore.
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
            tracing::info!(
                event = "run.finish",
                kind = "backup",
                run_id = %task_id,
                task_id = %task_id,
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
                println!(
                    "{}",
                    serde_json::json!({
                        "type": "task.state",
                        "taskId": task_id,
                        "kind": "backup",
                        "state": "succeeded",
                        "snapshotId": res.snapshot_id,
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
                    })
                );
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
                status = "failed",
                duration_seconds,
                error_code = e.code,
                error_message = %e.message,
                retryable = e.retryable,
                "run.finish"
            );
            Err(e)
        }
    }
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

    tracing::info!(
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

        let local_db_path = data_dir.join("index").join("index.sqlite");
        let (manifest_object_id, snapshot_provider) =
            lookup_manifest_meta(&local_db_path, &snapshot_id).await?;

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
            println!(
                "{}",
                serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "restore",
                    "state": "running",
                    "snapshotId": snapshot_id,
                })
            );
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
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
            tracing::info!(
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
                println!(
                    "{}",
                    serde_json::json!({
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
                    })
                );
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

    let bot_token = get_secret(config_dir, data_dir, &ep.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let master_key = load_master_key(config_dir, data_dir)?;
    let api_hash =
        get_secret(config_dir, data_dir, &settings.telegram.mtproto.api_hash_key)?.ok_or_else(|| {
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

    tracing::info!(
        event = "run.start",
        kind = "restore",
        run_id = %task_id,
        task_id = %task_id,
        snapshot_id = "latest",
        log_path = %run_log.path().display(),
        "run.start"
    );

    let started = std::time::Instant::now();
    let result: Result<(String, televy_backup_core::RestoreResult), CliError> = async {
        let settings = load_settings(config_dir)?;
        let t = select_target(&settings, target_id.as_deref(), source_path.as_deref())?;
        let ep = settings
            .telegram_endpoints
            .iter()
            .find(|e| e.id == t.endpoint_id)
            .ok_or_else(|| {
                CliError::new(
                    "config.invalid",
                    format!(
                        "target references unknown endpoint_id: target_id={} endpoint_id={}",
                        t.id, t.endpoint_id
                    ),
                )
            })?;

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
        let api_hash = get_secret(config_dir, data_dir, &settings.telegram.mtproto.api_hash_key)?
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

        let cache_db = data_dir
            .join("cache")
            .join("remote-index")
            .join(format!("{}.sqlite", latest.snapshot_id));
        if let Some(parent) = cache_db.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
        }

        if events {
            println!(
                "{}",
                serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "restore",
                    "state": "running",
                    "snapshotId": latest.snapshot_id,
                })
            );
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
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
            tracing::info!(
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
                println!(
                    "{}",
                    serde_json::json!({
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
                    })
                );
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
                snapshot_id = "latest",
                status = "failed",
                duration_seconds,
                error_code = e.code,
                error_message = %e.message,
                retryable = e.retryable,
                "run.finish"
            );
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

    tracing::info!(
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

        let local_db_path = data_dir.join("index").join("index.sqlite");
        let (manifest_object_id, snapshot_provider) =
            lookup_manifest_meta(&local_db_path, &snapshot_id).await?;

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
            println!(
                "{}",
                serde_json::json!({
                    "type": "task.state",
                    "taskId": task_id,
                    "kind": "verify",
                    "state": "running",
                    "snapshotId": snapshot_id,
                })
            );
        }

        let sink = NdjsonProgressSink {
            task_id: task_id.clone(),
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
            tracing::info!(
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
                println!(
                    "{}",
                    serde_json::json!({
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
                    })
                );
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
    reader
        .read_line(&mut resp_line)
        .map_err(|e| {
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
            resp.error.unwrap_or_else(|| "daemon request failed".to_string()),
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
    resp.vault_key_b64.ok_or_else(|| {
        CliError::new("daemon.failed", "vault IPC missing vault_key_b64")
    })
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

fn set_secret(
    config_dir: &Path,
    data_dir: &Path,
    key: &str,
    value: &str,
) -> Result<(), CliError> {
    let vault_key = load_or_create_vault_key(data_dir)?;
    let path = televy_backup_core::secrets::secrets_path(config_dir);
    let mut store = televy_backup_core::secrets::load_secrets_store(&path, &vault_key)
        .map_err(map_secrets_store_err)?;
    store.set(key, value);
    televy_backup_core::secrets::save_secrets_store(&path, &vault_key, &store)
        .map_err(map_secrets_store_err)?;
    Ok(())
}

fn delete_secret(config_dir: &Path, data_dir: &Path, key: &str) -> Result<bool, CliError> {
    let vault_key = load_or_create_vault_key(data_dir)?;
    let path = televy_backup_core::secrets::secrets_path(config_dir);
    let mut store = televy_backup_core::secrets::load_secrets_store(&path, &vault_key)
        .map_err(map_secrets_store_err)?;
    let removed = store.remove(key);
    if removed {
        televy_backup_core::secrets::save_secrets_store(&path, &vault_key, &store)
            .map_err(map_secrets_store_err)?;
    }
    Ok(removed)
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
