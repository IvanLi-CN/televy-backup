use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use base64::Engine;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use televy_backup_core::{
    APP_NAME, BackupConfig, BackupOptions, ChunkingConfig, ProgressSink, RestoreConfig,
    RestoreOptions, TelegramBotApiStorage, TelegramBotApiStorageConfig, VerifyConfig,
    VerifyOptions, restore_snapshot_with, run_backup_with, verify_snapshot_with,
};

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
    Get,
    Set,
}

#[derive(Subcommand)]
enum SecretsCmd {
    SetTelegramBotToken,
    InitMasterKey,
}

#[derive(Subcommand)]
enum TelegramCmd {
    Validate,
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
}

#[derive(Subcommand)]
enum BackupCmd {
    Run {
        #[arg(long)]
        source: PathBuf,
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
}

#[derive(Subcommand)]
enum VerifyCmd {
    Run {
        #[arg(long)]
        snapshot_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Settings {
    sources: Vec<String>,
    schedule: Schedule,
    retention: Retention,
    chunking: Chunking,
    telegram: Telegram,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Schedule {
    enabled: bool,
    kind: String,
    hourly_minute: u8,
    daily_at: String,
    timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Retention {
    keep_last_snapshots: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Chunking {
    min_bytes: u32,
    avg_bytes: u32,
    max_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Telegram {
    mode: String,
    chat_id: String,
    bot_token_key: String,
    rate_limit: TelegramRateLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramRateLimit {
    max_concurrent_uploads: u32,
    min_delay_ms: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sources: vec![],
            schedule: Schedule {
                enabled: false,
                kind: "hourly".to_string(),
                hourly_minute: 0,
                daily_at: "02:00".to_string(),
                timezone: "local".to_string(),
            },
            retention: Retention {
                keep_last_snapshots: 7,
            },
            chunking: Chunking {
                min_bytes: 1024 * 1024,
                avg_bytes: 4 * 1024 * 1024,
                max_bytes: 10 * 1024 * 1024,
            },
            telegram: Telegram {
                mode: "botapi".to_string(),
                chat_id: "".to_string(),
                bot_token_key: "telegram.bot_token".to_string(),
                rate_limit: TelegramRateLimit {
                    max_concurrent_uploads: 2,
                    min_delay_ms: 250,
                },
            },
        }
    }
}

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
}

struct NdjsonProgressSink {
    task_id: String,
}

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
            SettingsCmd::Get => settings_get(&config_dir, cli.json).await,
            SettingsCmd::Set => settings_set(&config_dir, cli.json).await,
        },
        Command::Secrets { cmd } => match cmd {
            SecretsCmd::SetTelegramBotToken => {
                secrets_set_telegram_bot_token(&config_dir, cli.json).await
            }
            SecretsCmd::InitMasterKey => secrets_init_master_key(cli.json).await,
        },
        Command::Telegram { cmd } => match cmd {
            TelegramCmd::Validate => telegram_validate(&config_dir, cli.json).await,
        },
        Command::Snapshots { cmd } => match cmd {
            SnapshotsCmd::List { limit } => snapshots_list(&data_dir, limit, cli.json).await,
        },
        Command::Stats { cmd } => match cmd {
            StatsCmd::Get => stats_get(&data_dir, cli.json).await,
        },
        Command::Backup { cmd } => match cmd {
            BackupCmd::Run { source, label } => {
                backup_run(&config_dir, &data_dir, source, label, cli.json, cli.events).await
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
        },
        Command::Verify { cmd } => match cmd {
            VerifyCmd::Run { snapshot_id } => {
                verify_run(&config_dir, &data_dir, snapshot_id, cli.json, cli.events).await
            }
        },
    }
}

async fn settings_get(config_dir: &Path, json: bool) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let telegram_present = get_secret(&settings.telegram.bot_token_key)?.is_some();
    let master_present = get_secret(MASTER_KEY_KEY)?.is_some();

    if json {
        println!(
            "{}",
            serde_json::json!({
                "settings": settings,
                "secrets": { "telegramBotTokenPresent": telegram_present, "masterKeyPresent": master_present }
            })
        );
    } else {
        let text = toml::to_string(&settings)
            .map_err(|e| CliError::new("config.invalid", e.to_string()))?;
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
        println!();
        println!("telegramBotTokenPresent={telegram_present}");
        println!("masterKeyPresent={master_present}");
    }
    Ok(())
}

async fn settings_set(config_dir: &Path, json: bool) -> Result<(), CliError> {
    let mut input = String::new();
    std::io::stdin()
        .read_to_string(&mut input)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let settings: Settings =
        toml::from_str(&input).map_err(|e| CliError::new("config.invalid", e.to_string()))?;
    validate_settings(&settings)?;
    save_settings(config_dir, &settings)?;

    if json {
        println!("{}", serde_json::json!({ "settings": settings }));
    }
    Ok(())
}

async fn secrets_set_telegram_bot_token(config_dir: &Path, json: bool) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    let mut token = String::new();
    std::io::stdin()
        .read_to_string(&mut token)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let token = token.trim().to_string();
    if token.is_empty() {
        return Err(CliError::new("config.invalid", "token is empty"));
    }
    set_secret(&settings.telegram.bot_token_key, &token)?;

    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn secrets_init_master_key(json: bool) -> Result<(), CliError> {
    if get_secret(MASTER_KEY_KEY)?.is_some() {
        return Err(CliError::new(
            "keychain.write_failed",
            "master key already exists",
        ));
    }
    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| CliError::new("keychain.write_failed", format!("getrandom failed: {e}")))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    set_secret(MASTER_KEY_KEY, &b64)?;
    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn telegram_validate(config_dir: &Path, json: bool) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    validate_settings(&settings)?;
    if settings.telegram.chat_id.is_empty() {
        return Err(CliError::new("config.invalid", "telegram.chat_id is empty"));
    }
    let token = get_secret(&settings.telegram.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;

    let client = reqwest::Client::new();
    let base = format!("https://api.telegram.org/bot{token}");

    let me: TelegramResponse<TelegramMeResult> = client
        .get(format!("{base}/getMe"))
        .send()
        .await
        .map_err(|e| CliError::retryable("telegram.unavailable", format!("getMe failed: {e}")))?
        .json()
        .await
        .map_err(|e| CliError::new("telegram.unavailable", format!("getMe json failed: {e}")))?;
    if !me.ok {
        return Err(CliError::new(
            "telegram.unauthorized",
            me.description
                .unwrap_or_else(|| "telegram returned ok=false".to_string()),
        ));
    }
    let bot_username = me.result.username.unwrap_or_default();

    let chat: TelegramResponse<TelegramChatResult> = client
        .get(format!("{base}/getChat"))
        .query(&[("chat_id", settings.telegram.chat_id.clone())])
        .send()
        .await
        .map_err(|e| CliError::retryable("telegram.unavailable", format!("getChat failed: {e}")))?
        .json()
        .await
        .map_err(|e| CliError::new("telegram.unavailable", format!("getChat json failed: {e}")))?;
    if !chat.ok {
        let msg = chat
            .description
            .unwrap_or_else(|| "telegram returned ok=false".to_string());
        return Err(CliError::new("telegram.chat_not_found", msg));
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "botUsername": bot_username,
                "chatId": settings.telegram.chat_id,
            })
        );
    } else {
        println!("botUsername={bot_username}");
        println!("chatId={}", settings.telegram.chat_id);
    }
    Ok(())
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

async fn backup_run(
    config_dir: &Path,
    data_dir: &Path,
    source: PathBuf,
    label: String,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    validate_settings(&settings)?;

    let token = get_secret(&settings.telegram.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let master_key = load_master_key()?;

    if settings.telegram.chat_id.is_empty() {
        return Err(CliError::new("config.invalid", "telegram.chat_id is empty"));
    }
    let storage = TelegramBotApiStorage::new(TelegramBotApiStorageConfig {
        bot_token: token,
        chat_id: settings.telegram.chat_id.clone(),
    });

    let db_path = data_dir.join("index").join("index.sqlite");
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
    }

    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
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
    let res = run_backup_with(
        &storage,
        BackupConfig {
            db_path,
            source_path: source,
            label,
            chunking: ChunkingConfig {
                min_bytes: settings.chunking.min_bytes,
                avg_bytes: settings.chunking.avg_bytes,
                max_bytes: settings.chunking.max_bytes,
            },
            master_key,
            snapshot_id: None,
            keep_last_snapshots: settings.retention.keep_last_snapshots,
        },
        opts,
    )
    .await
    .map_err(map_core_err)?;

    if events {
        println!(
            "{}",
            serde_json::json!({
                "type": "task.state",
                "taskId": task_id,
                "kind": "backup",
                "state": "succeeded",
                "snapshotId": res.snapshot_id,
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
            "filesIndexed={} chunksUploaded={} bytesUploaded={} bytesDeduped={}",
            res.files_indexed, res.chunks_uploaded, res.bytes_uploaded, res.bytes_deduped
        );
    }
    Ok(())
}

async fn restore_run(
    config_dir: &Path,
    data_dir: &Path,
    snapshot_id: String,
    target: PathBuf,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    validate_settings(&settings)?;

    let token = get_secret(&settings.telegram.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let master_key = load_master_key()?;
    if settings.telegram.chat_id.is_empty() {
        return Err(CliError::new("config.invalid", "telegram.chat_id is empty"));
    }
    let storage = TelegramBotApiStorage::new(TelegramBotApiStorageConfig {
        bot_token: token,
        chat_id: settings.telegram.chat_id.clone(),
    });

    let local_db_path = data_dir.join("index").join("index.sqlite");
    let manifest_object_id = lookup_manifest_object_id(&local_db_path, &snapshot_id).await?;

    let cache_db = data_dir
        .join("cache")
        .join("remote-index")
        .join(format!("{snapshot_id}.sqlite"));
    if let Some(parent) = cache_db.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
    }

    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
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
    let _res = restore_snapshot_with(
        &storage,
        RestoreConfig {
            snapshot_id: snapshot_id.clone(),
            manifest_object_id,
            master_key,
            index_db_path: cache_db,
            target_path: target,
        },
        opts,
    )
    .await
    .map_err(map_core_err)?;

    if events {
        println!(
            "{}",
            serde_json::json!({
                "type": "task.state",
                "taskId": task_id,
                "kind": "restore",
                "state": "succeeded",
                "snapshotId": snapshot_id,
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

async fn verify_run(
    config_dir: &Path,
    data_dir: &Path,
    snapshot_id: String,
    json: bool,
    events: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    validate_settings(&settings)?;

    let token = get_secret(&settings.telegram.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let master_key = load_master_key()?;
    if settings.telegram.chat_id.is_empty() {
        return Err(CliError::new("config.invalid", "telegram.chat_id is empty"));
    }
    let storage = TelegramBotApiStorage::new(TelegramBotApiStorageConfig {
        bot_token: token,
        chat_id: settings.telegram.chat_id.clone(),
    });

    let local_db_path = data_dir.join("index").join("index.sqlite");
    let manifest_object_id = lookup_manifest_object_id(&local_db_path, &snapshot_id).await?;

    let cache_db = data_dir
        .join("cache")
        .join("remote-index")
        .join(format!("{snapshot_id}.sqlite"));
    if let Some(parent) = cache_db.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
    }

    let task_id = format!("tsk_{}", uuid::Uuid::new_v4());
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
    let _res = verify_snapshot_with(
        &storage,
        VerifyConfig {
            snapshot_id: snapshot_id.clone(),
            manifest_object_id,
            master_key,
            index_db_path: cache_db,
        },
        opts,
    )
    .await
    .map_err(map_core_err)?;

    if events {
        println!(
            "{}",
            serde_json::json!({
                "type": "task.state",
                "taskId": task_id,
                "kind": "verify",
                "state": "succeeded",
                "snapshotId": snapshot_id,
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

async fn lookup_manifest_object_id(db_path: &Path, snapshot_id: &str) -> Result<String, CliError> {
    let pool = televy_backup_core::index_db::open_existing_index_db(db_path)
        .await
        .map_err(map_core_err)?;

    let row =
        sqlx::query("SELECT manifest_object_id FROM remote_indexes WHERE snapshot_id = ? LIMIT 1")
            .bind(snapshot_id)
            .fetch_optional(&pool)
            .await
            .map_err(|e| CliError::new("db.failed", e.to_string()))?;

    match row {
        Some(r) => Ok(r.get::<String, _>("manifest_object_id")),
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

fn config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("config.toml")
}

fn load_settings(config_dir: &Path) -> Result<Settings, CliError> {
    let path = config_path(config_dir);
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| CliError::new("config.read_failed", e.to_string()))?;
    let s: Settings =
        toml::from_str(&text).map_err(|e| CliError::new("config.invalid", e.to_string()))?;
    Ok(s)
}

fn save_settings(config_dir: &Path, settings: &Settings) -> Result<(), CliError> {
    validate_settings(settings)?;
    let path = config_path(config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
    }
    let text =
        toml::to_string(settings).map_err(|e| CliError::new("config.invalid", e.to_string()))?;
    atomic_write(&path, text.as_bytes())
        .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;
    Ok(())
}

fn validate_settings(settings: &Settings) -> Result<(), CliError> {
    if settings.telegram.mode != "botapi" {
        return Err(CliError::new(
            "config.invalid",
            "telegram.mode must be \"botapi\"",
        ));
    }
    if settings.telegram.bot_token_key.is_empty() {
        return Err(CliError::new(
            "config.invalid",
            "telegram.bot_token_key must not be empty",
        ));
    }
    if settings.retention.keep_last_snapshots < 1 {
        return Err(CliError::new(
            "config.invalid",
            "retention.keep_last_snapshots must be >= 1",
        ));
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn get_secret(key: &str) -> Result<Option<String>, CliError> {
    let entry = keyring::Entry::new(APP_NAME, key)
        .map_err(|e| CliError::new("keychain.unavailable", e.to_string()))?;
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(CliError::new("keychain.unavailable", e.to_string())),
    }
}

#[cfg(not(target_os = "macos"))]
fn get_secret(_key: &str) -> Result<Option<String>, CliError> {
    Err(CliError::new(
        "keychain.unavailable",
        "keychain only supported on macOS",
    ))
}

#[cfg(target_os = "macos")]
fn set_secret(key: &str, value: &str) -> Result<(), CliError> {
    let entry = keyring::Entry::new(APP_NAME, key)
        .map_err(|e| CliError::new("keychain.unavailable", e.to_string()))?;
    entry
        .set_password(value)
        .map_err(|e| CliError::new("keychain.write_failed", e.to_string()))?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn set_secret(_key: &str, _value: &str) -> Result<(), CliError> {
    Err(CliError::new(
        "keychain.unavailable",
        "keychain only supported on macOS",
    ))
}

fn load_master_key() -> Result<[u8; 32], CliError> {
    let b64 = get_secret(MASTER_KEY_KEY)?
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

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: T,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramMeResult {
    username: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramChatResult {}
