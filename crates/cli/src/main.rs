use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use base64::Engine;
use clap::{Parser, Subcommand};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use televy_backup_core::{
    APP_NAME, BackupConfig, BackupOptions, ChunkingConfig, ProgressSink, RestoreConfig,
    RestoreOptions, Storage, TelegramMtProtoStorage, TelegramMtProtoStorageConfig, VerifyConfig,
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
    Get {
        #[arg(long)]
        with_secrets: bool,
    },
    Set,
}

#[derive(Subcommand)]
enum SecretsCmd {
    SetTelegramBotToken,
    SetTelegramApiHash,
    ClearTelegramMtprotoSession,
    MigrateKeychain,
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
    Last {
        #[arg(long)]
        source: Option<PathBuf>,
    },
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
    #[serde(default)]
    mtproto: TelegramMtproto,
    rate_limit: TelegramRateLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramMtproto {
    api_id: i32,
    api_hash_key: String,
    session_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TelegramRateLimit {
    max_concurrent_uploads: u32,
    min_delay_ms: u32,
}

impl Default for TelegramMtproto {
    fn default() -> Self {
        Self {
            api_id: 0,
            api_hash_key: "telegram.mtproto.api_hash".to_string(),
            session_key: "telegram.mtproto.session".to_string(),
        }
    }
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
                mode: "mtproto".to_string(),
                chat_id: "".to_string(),
                bot_token_key: "telegram.bot_token".to_string(),
                mtproto: TelegramMtproto::default(),
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
                settings_get(&config_dir, cli.json, with_secrets).await
            }
            SettingsCmd::Set => settings_set(&config_dir, cli.json).await,
        },
        Command::Secrets { cmd } => match cmd {
            SecretsCmd::SetTelegramBotToken => {
                secrets_set_telegram_bot_token(&config_dir, cli.json).await
            }
            SecretsCmd::SetTelegramApiHash => {
                secrets_set_telegram_api_hash(&config_dir, cli.json).await
            }
            SecretsCmd::ClearTelegramMtprotoSession => {
                secrets_clear_telegram_mtproto_session(&config_dir, cli.json).await
            }
            SecretsCmd::MigrateKeychain => secrets_migrate_keychain(&config_dir, cli.json).await,
            SecretsCmd::InitMasterKey => secrets_init_master_key(&config_dir, cli.json).await,
        },
        Command::Telegram { cmd } => match cmd {
            TelegramCmd::Validate => telegram_validate(&config_dir, &data_dir, cli.json).await,
        },
        Command::Snapshots { cmd } => match cmd {
            SnapshotsCmd::List { limit } => snapshots_list(&data_dir, limit, cli.json).await,
        },
        Command::Stats { cmd } => match cmd {
            StatsCmd::Get => stats_get(&data_dir, cli.json).await,
            StatsCmd::Last { source } => stats_last(&data_dir, source, cli.json).await,
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

async fn settings_get(config_dir: &Path, json: bool, with_secrets: bool) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;

    if json {
        if with_secrets {
            let telegram_present =
                get_secret(config_dir, &settings.telegram.bot_token_key)?.is_some();
            let master_present = get_secret(config_dir, MASTER_KEY_KEY)?.is_some();
            let mtproto_api_hash_present =
                get_secret(config_dir, &settings.telegram.mtproto.api_hash_key)?.is_some();
            let mtproto_session_present =
                get_secret(config_dir, &settings.telegram.mtproto.session_key)?.is_some();
            println!(
                "{}",
                serde_json::json!({
                    "settings": settings,
                    "secrets": {
                        "telegramBotTokenPresent": telegram_present,
                        "masterKeyPresent": master_present,
                        "telegramMtprotoApiHashPresent": mtproto_api_hash_present,
                        "telegramMtprotoSessionPresent": mtproto_session_present
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
            let telegram_present =
                get_secret(config_dir, &settings.telegram.bot_token_key)?.is_some();
            let master_present = get_secret(config_dir, MASTER_KEY_KEY)?.is_some();
            let mtproto_api_hash_present =
                get_secret(config_dir, &settings.telegram.mtproto.api_hash_key)?.is_some();
            let mtproto_session_present =
                get_secret(config_dir, &settings.telegram.mtproto.session_key)?.is_some();
            println!();
            println!("telegramBotTokenPresent={telegram_present}");
            println!("masterKeyPresent={master_present}");
            println!("telegramMtprotoApiHashPresent={mtproto_api_hash_present}");
            println!("telegramMtprotoSessionPresent={mtproto_session_present}");
        }
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
    set_secret(config_dir, &settings.telegram.bot_token_key, &token)?;

    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn secrets_set_telegram_api_hash(config_dir: &Path, json: bool) -> Result<(), CliError> {
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
    json: bool,
) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    delete_secret(config_dir, &settings.telegram.mtproto.session_key)?;

    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn secrets_migrate_keychain(config_dir: &Path, json: bool) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;

    let mut migrated = Vec::<String>::new();
    let mut deleted = Vec::<String>::new();
    let mut conflicts = Vec::<String>::new();

    let bot_key = settings.telegram.bot_token_key.clone();
    if let Some(token) = keychain_get_secret(&bot_key)? {
        let store_val = get_secret(config_dir, &bot_key)?;
        match store_val {
            None => {
                set_secret(config_dir, &bot_key, &token)?;
                migrated.push(bot_key.clone());
                if keychain_delete_secret(&bot_key)? {
                    deleted.push(bot_key.clone());
                }
            }
            Some(existing) => {
                if existing == token {
                    if keychain_delete_secret(&bot_key)? {
                        deleted.push(bot_key.clone());
                    }
                } else {
                    conflicts.push(bot_key.clone());
                }
            }
        }
    }

    if let Some(master_key) = keychain_get_secret(MASTER_KEY_KEY)? {
        let store_val = get_secret(config_dir, MASTER_KEY_KEY)?;
        match store_val {
            None => {
                set_secret(config_dir, MASTER_KEY_KEY, &master_key)?;
                migrated.push(MASTER_KEY_KEY.to_string());
                if keychain_delete_secret(MASTER_KEY_KEY)? {
                    deleted.push(MASTER_KEY_KEY.to_string());
                }
            }
            Some(existing) => {
                if existing == master_key {
                    if keychain_delete_secret(MASTER_KEY_KEY)? {
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

async fn secrets_init_master_key(config_dir: &Path, json: bool) -> Result<(), CliError> {
    if get_secret(config_dir, MASTER_KEY_KEY)?.is_some() {
        return Err(CliError::new(
            "secrets.store_failed",
            "master key already exists",
        ));
    }

    #[cfg(target_os = "macos")]
    {
        if keychain_get_secret(MASTER_KEY_KEY)?.is_some() {
            return Err(CliError::new(
                "secrets.store_failed",
                "master key exists in Keychain (old scheme). Fix: run `televybackup secrets migrate-keychain` instead of generating a new one.",
            ));
        }
    }

    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| CliError::new("secrets.store_failed", format!("getrandom failed: {e}")))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    set_secret(config_dir, MASTER_KEY_KEY, &b64)?;
    if json {
        println!("{}", serde_json::json!({ "ok": true }));
    } else {
        println!("ok");
    }
    Ok(())
}

async fn telegram_validate(config_dir: &Path, data_dir: &Path, json: bool) -> Result<(), CliError> {
    let settings = load_settings(config_dir)?;
    validate_settings(&settings)?;
    if settings.telegram.chat_id.is_empty() {
        return Err(CliError::new("config.invalid", "telegram.chat_id is empty"));
    }

    let bot_token = get_secret(config_dir, &settings.telegram.bot_token_key)?
        .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
    let api_hash =
        get_secret(config_dir, &settings.telegram.mtproto.api_hash_key)?.ok_or_else(|| {
            CliError::new(
                "telegram.mtproto.missing_api_hash",
                "mtproto api_hash missing",
            )
        })?;

    let session = load_optional_base64_secret_bytes(
        config_dir,
        &settings.telegram.mtproto.session_key,
        "telegram.mtproto.session_invalid",
        "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
    )?;

    let cache_dir = data_dir.join("cache").join("mtproto");
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

    let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
        api_id: settings.telegram.mtproto.api_id,
        api_hash: api_hash.clone(),
        bot_token: bot_token.clone(),
        chat_id: settings.telegram.chat_id.clone(),
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
        set_secret(config_dir, &settings.telegram.mtproto.session_key, &b64)?;
    }

    if json {
        println!(
            "{}",
            serde_json::json!({
                "mode": "mtproto",
                "chatId": settings.telegram.chat_id,
                "roundTripOk": true,
                "sampleObjectId": object_id,
            })
        );
    } else {
        println!("mode=mtproto");
        println!("chatId={}", settings.telegram.chat_id);
        println!("roundTripOk=true");
        println!("sampleObjectId={object_id}");
    }
    Ok(())
}

fn load_optional_base64_secret_bytes(
    config_dir: &Path,
    key: &str,
    error_code: &'static str,
    error_message: &str,
) -> Result<Option<Vec<u8>>, CliError> {
    let Some(b64) = get_secret(config_dir, key)? else {
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
    source: PathBuf,
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
        validate_settings(&settings)?;

        let bot_token = get_secret(config_dir, &settings.telegram.bot_token_key)?
            .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
        let master_key = load_master_key(config_dir)?;

        if settings.telegram.chat_id.is_empty() {
            return Err(CliError::new("config.invalid", "telegram.chat_id is empty"));
        }

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
        };

        let api_hash = get_secret(config_dir, &settings.telegram.mtproto.api_hash_key)?
            .ok_or_else(|| {
                CliError::new(
                    "telegram.mtproto.missing_api_hash",
                    "mtproto api_hash missing",
                )
            })?;
        let session = load_optional_base64_secret_bytes(
            config_dir,
            &settings.telegram.mtproto.session_key,
            "telegram.mtproto.session_invalid",
            "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
        )?;

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            api_id: settings.telegram.mtproto.api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: settings.telegram.chat_id.clone(),
            session,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(map_core_err)?;

        let res = run_backup_with(&storage, cfg, opts)
            .await
            .map_err(map_core_err)?;

        if let Some(bytes) = storage.session_bytes() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            if let Err(e) = set_secret(config_dir, &settings.telegram.mtproto.session_key, &b64) {
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
        validate_settings(&settings)?;

        if settings.telegram.chat_id.is_empty() {
            return Err(CliError::new("config.invalid", "telegram.chat_id is empty"));
        }

        let local_db_path = data_dir.join("index").join("index.sqlite");
        let (manifest_object_id, snapshot_provider) =
            lookup_manifest_meta(&local_db_path, &snapshot_id).await?;

        if snapshot_provider != "telegram.mtproto" {
            return Err(CliError::new(
                "snapshot.unsupported_provider",
                format!(
                    "unsupported snapshot provider: snapshot_id={snapshot_id} provider={snapshot_provider}. TelevyBackup is MTProto-only now. Fix: run a new backup with MTProto."
                ),
            ));
        }

        let bot_token = get_secret(config_dir, &settings.telegram.bot_token_key)?
            .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
        let master_key = load_master_key(config_dir)?;

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

        let api_hash = get_secret(config_dir, &settings.telegram.mtproto.api_hash_key)?.ok_or_else(
            || CliError::new("telegram.mtproto.missing_api_hash", "mtproto api_hash missing"),
        )?;
        let session = load_optional_base64_secret_bytes(
            config_dir,
            &settings.telegram.mtproto.session_key,
            "telegram.mtproto.session_invalid",
            "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
        )?;

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            api_id: settings.telegram.mtproto.api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: settings.telegram.chat_id.clone(),
            session,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(map_core_err)?;

        let res = restore_snapshot_with(&storage, cfg, opts).await.map_err(map_core_err)?;

        if let Some(bytes) = storage.session_bytes() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            if let Err(e) = set_secret(config_dir, &settings.telegram.mtproto.session_key, &b64) {
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
        validate_settings(&settings)?;

        if settings.telegram.chat_id.is_empty() {
            return Err(CliError::new("config.invalid", "telegram.chat_id is empty"));
        }

        let local_db_path = data_dir.join("index").join("index.sqlite");
        let (manifest_object_id, snapshot_provider) =
            lookup_manifest_meta(&local_db_path, &snapshot_id).await?;

        if snapshot_provider != "telegram.mtproto" {
            return Err(CliError::new(
                "snapshot.unsupported_provider",
                format!(
                    "unsupported snapshot provider: snapshot_id={snapshot_id} provider={snapshot_provider}. TelevyBackup is MTProto-only now. Fix: run a new backup with MTProto."
                ),
            ));
        }

        let bot_token = get_secret(config_dir, &settings.telegram.bot_token_key)?
            .ok_or_else(|| CliError::new("telegram.unauthorized", "bot token missing"))?;
        let master_key = load_master_key(config_dir)?;

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

        let api_hash = get_secret(config_dir, &settings.telegram.mtproto.api_hash_key)?.ok_or_else(
            || CliError::new("telegram.mtproto.missing_api_hash", "mtproto api_hash missing"),
        )?;
        let session = load_optional_base64_secret_bytes(
            config_dir,
            &settings.telegram.mtproto.session_key,
            "telegram.mtproto.session_invalid",
            "invalid mtproto session (try: `televybackup secrets clear-telegram-mtproto-session`)",
        )?;

        let cache_dir = data_dir.join("cache").join("mtproto");
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| CliError::new("config.write_failed", e.to_string()))?;

        let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            api_id: settings.telegram.mtproto.api_id,
            api_hash: api_hash.clone(),
            bot_token: bot_token.clone(),
            chat_id: settings.telegram.chat_id.clone(),
            session,
            cache_dir,
            helper_path: None,
        })
        .await
        .map_err(map_core_err)?;

        let res = verify_snapshot_with(&storage, cfg, opts).await.map_err(map_core_err)?;

        if let Some(bytes) = storage.session_bytes() {
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            if let Err(e) = set_secret(config_dir, &settings.telegram.mtproto.session_key, &b64) {
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
    let mut s: Settings =
        toml::from_str(&text).map_err(|e| CliError::new("config.invalid", e.to_string()))?;

    // MTProto-only migration: transparently upgrade old botapi configs so UI/CLI stop showing botapi.
    if s.telegram.mode.trim() != "mtproto" {
        s.telegram.mode = "mtproto".to_string();
        if let Ok(new_text) = toml::to_string(&s) {
            let _ = atomic_write(&path, new_text.as_bytes());
        }
    }
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
    match settings.telegram.mode.as_str() {
        "mtproto" => {
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
            if settings.telegram.mtproto.session_key.is_empty() {
                return Err(CliError::new(
                    "config.invalid",
                    "telegram.mtproto.session_key must not be empty",
                ));
            }
        }
        other => {
            return Err(CliError::new(
                "config.invalid",
                format!(
                    "telegram.mode must be \"mtproto\" (got {other:?}); Telegram Bot API is no longer supported"
                ),
            ));
        }
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

fn redact_secret(s: impl Into<String>, secret: &str) -> String {
    let s = s.into();
    if secret.is_empty() {
        s
    } else {
        s.replace(secret, "[redacted]")
    }
}

#[cfg(target_os = "macos")]
fn keychain_get_secret(key: &str) -> Result<Option<String>, CliError> {
    use security_framework::passwords::{PasswordOptions, generic_password};

    let opts = PasswordOptions::new_generic_password(APP_NAME, key);
    match generic_password(opts) {
        Ok(bytes) => {
            let s = String::from_utf8(bytes).map_err(|e| {
                CliError::new("keychain.unavailable", format!("utf8 decode failed: {e}"))
            })?;
            Ok(Some(s))
        }
        Err(e) => {
            if is_keychain_not_found(&e) {
                return Ok(None);
            }
            Err(CliError::new("keychain.unavailable", e.to_string()))
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn keychain_get_secret(_key: &str) -> Result<Option<String>, CliError> {
    Err(CliError::new(
        "keychain.unavailable",
        "keychain only supported on macOS",
    ))
}

#[cfg(target_os = "macos")]
fn keychain_set_secret(key: &str, value: &str) -> Result<(), CliError> {
    use security_framework::passwords::set_generic_password;
    set_generic_password(APP_NAME, key, value.as_bytes())
        .map_err(|e| CliError::new("keychain.unavailable", e.to_string()))?;
    Ok(())
}

#[cfg(not(target_os = "macos"))]
fn keychain_set_secret(_key: &str, _value: &str) -> Result<(), CliError> {
    Err(CliError::new(
        "keychain.unavailable",
        "keychain only supported on macOS",
    ))
}

#[cfg(target_os = "macos")]
fn keychain_delete_secret(key: &str) -> Result<bool, CliError> {
    use security_framework::passwords::delete_generic_password;

    match delete_generic_password(APP_NAME, key) {
        Ok(()) => Ok(true),
        Err(e) => {
            if is_keychain_not_found(&e) {
                return Ok(false);
            }
            Err(CliError::new("keychain.unavailable", e.to_string()))
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn keychain_delete_secret(_key: &str) -> Result<bool, CliError> {
    Err(CliError::new(
        "keychain.unavailable",
        "keychain only supported on macOS",
    ))
}

#[cfg(target_os = "macos")]
fn is_keychain_not_found(e: &security_framework::base::Error) -> bool {
    // errSecItemNotFound
    e.code() == -25300
}

#[cfg(target_os = "macos")]
fn load_or_create_vault_key() -> Result<[u8; 32], CliError> {
    if let Some(key) = VAULT_KEY_CACHE.get() {
        return Ok(*key);
    }

    let key = load_or_create_vault_key_uncached()?;
    let _ = VAULT_KEY_CACHE.set(key);
    Ok(key)
}

#[cfg(target_os = "macos")]
fn load_or_create_vault_key_uncached() -> Result<[u8; 32], CliError> {
    let existing = keychain_get_secret(televy_backup_core::secrets::VAULT_KEY_KEY)
        .map_err(|e| CliError::new("secrets.vault_unavailable", e.message))?;

    if let Some(b64) = existing {
        return televy_backup_core::secrets::vault_key_from_base64(&b64)
            .map_err(|e| CliError::new("secrets.vault_unavailable", e.to_string()));
    }

    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes).map_err(|e| {
        CliError::new(
            "secrets.vault_unavailable",
            format!("getrandom failed: {e}"),
        )
    })?;
    let b64 = televy_backup_core::secrets::vault_key_to_base64(&bytes);
    keychain_set_secret(televy_backup_core::secrets::VAULT_KEY_KEY, &b64)
        .map_err(|e| CliError::new("secrets.vault_unavailable", e.message))?;

    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
fn load_or_create_vault_key() -> Result<[u8; 32], CliError> {
    Err(CliError::new(
        "keychain.unavailable",
        "keychain only supported on macOS",
    ))
}

fn get_secret(config_dir: &Path, key: &str) -> Result<Option<String>, CliError> {
    let vault_key = load_or_create_vault_key()?;
    let path = televy_backup_core::secrets::secrets_path(config_dir);
    let store = televy_backup_core::secrets::load_secrets_store(&path, &vault_key)
        .map_err(map_secrets_store_err)?;
    Ok(store.get(key).map(|s| s.to_string()))
}

fn set_secret(config_dir: &Path, key: &str, value: &str) -> Result<(), CliError> {
    let vault_key = load_or_create_vault_key()?;
    let path = televy_backup_core::secrets::secrets_path(config_dir);
    let mut store = televy_backup_core::secrets::load_secrets_store(&path, &vault_key)
        .map_err(map_secrets_store_err)?;
    store.set(key, value);
    televy_backup_core::secrets::save_secrets_store(&path, &vault_key, &store)
        .map_err(map_secrets_store_err)?;
    Ok(())
}

fn delete_secret(config_dir: &Path, key: &str) -> Result<bool, CliError> {
    let vault_key = load_or_create_vault_key()?;
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

fn load_master_key(config_dir: &Path) -> Result<[u8; 32], CliError> {
    let b64 = get_secret(config_dir, MASTER_KEY_KEY)?
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
