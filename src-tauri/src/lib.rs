mod app_config;
mod rpc;
mod secrets;
mod task_manager;

use std::sync::Arc;

use base64::Engine;
use sqlx::Row;

use crate::app_config::{MASTER_KEY_KEY, Settings};
use crate::rpc::RpcError;
use crate::task_manager::{TaskManager, TaskProgressEvent};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcSettings {
    sources: Vec<String>,
    schedule: RpcSchedule,
    retention: RpcRetention,
    chunking: RpcChunking,
    telegram: RpcTelegram,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcSchedule {
    enabled: bool,
    kind: String,
    hourly_minute: u8,
    daily_at: String,
    timezone: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcRetention {
    keep_last_snapshots: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcChunking {
    min_bytes: u32,
    avg_bytes: u32,
    max_bytes: u32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcTelegram {
    mode: String,
    chat_id: String,
    bot_token_key: String,
    rate_limit: RpcTelegramRateLimit,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RpcTelegramRateLimit {
    max_concurrent_uploads: u32,
    min_delay_ms: u32,
}

impl From<Settings> for RpcSettings {
    fn from(s: Settings) -> Self {
        Self {
            sources: s.sources,
            schedule: RpcSchedule {
                enabled: s.schedule.enabled,
                kind: s.schedule.kind,
                hourly_minute: s.schedule.hourly_minute,
                daily_at: s.schedule.daily_at,
                timezone: s.schedule.timezone,
            },
            retention: RpcRetention {
                keep_last_snapshots: s.retention.keep_last_snapshots,
            },
            chunking: RpcChunking {
                min_bytes: s.chunking.min_bytes,
                avg_bytes: s.chunking.avg_bytes,
                max_bytes: s.chunking.max_bytes,
            },
            telegram: RpcTelegram {
                mode: s.telegram.mode,
                chat_id: s.telegram.chat_id,
                bot_token_key: s.telegram.bot_token_key,
                rate_limit: RpcTelegramRateLimit {
                    max_concurrent_uploads: s.telegram.rate_limit.max_concurrent_uploads,
                    min_delay_ms: s.telegram.rate_limit.min_delay_ms,
                },
            },
        }
    }
}

impl From<RpcSettings> for Settings {
    fn from(s: RpcSettings) -> Self {
        Self {
            sources: s.sources,
            schedule: app_config::Schedule {
                enabled: s.schedule.enabled,
                kind: s.schedule.kind,
                hourly_minute: s.schedule.hourly_minute,
                daily_at: s.schedule.daily_at,
                timezone: s.schedule.timezone,
            },
            retention: app_config::Retention {
                keep_last_snapshots: s.retention.keep_last_snapshots,
            },
            chunking: app_config::Chunking {
                min_bytes: s.chunking.min_bytes,
                avg_bytes: s.chunking.avg_bytes,
                max_bytes: s.chunking.max_bytes,
            },
            telegram: app_config::Telegram {
                mode: s.telegram.mode,
                chat_id: s.telegram.chat_id,
                bot_token_key: s.telegram.bot_token_key,
                rate_limit: app_config::TelegramRateLimit {
                    max_concurrent_uploads: s.telegram.rate_limit.max_concurrent_uploads,
                    min_delay_ms: s.telegram.rate_limit.min_delay_ms,
                },
            },
        }
    }
}

#[tauri::command]
async fn ping(value: String) -> Result<String, RpcError> {
    Ok(format!("pong: {value}"))
}

#[derive(Debug, serde::Serialize)]
struct SettingsGetResponse {
    settings: RpcSettings,
    secrets: SecretsStatus,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct SecretsStatus {
    telegram_bot_token_present: bool,
    master_key_present: bool,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsSetRequest {
    settings: RpcSettings,
    secrets: SettingsSecrets,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsSecrets {
    telegram_bot_token: Option<String>,
    rotate_master_key: bool,
}

#[tauri::command]
async fn settings_get(app: tauri::AppHandle) -> Result<SettingsGetResponse, RpcError> {
    let settings = app_config::load_settings(&app)?;
    let telegram_bot_token_present =
        secrets::get_secret(&settings.telegram.bot_token_key)?.is_some();
    let master_key_present = secrets::get_secret(MASTER_KEY_KEY)?.is_some();

    Ok(SettingsGetResponse {
        settings: RpcSettings::from(settings),
        secrets: SecretsStatus {
            telegram_bot_token_present,
            master_key_present,
        },
    })
}

#[tauri::command]
async fn settings_set(
    app: tauri::AppHandle,
    req: SettingsSetRequest,
) -> Result<RpcSettings, RpcError> {
    let settings: Settings = req.settings.into();
    app_config::save_settings(&app, &settings)?;

    if let Some(token) = req.secrets.telegram_bot_token {
        secrets::set_secret(&settings.telegram.bot_token_key, &token)?;
    }
    if req.secrets.rotate_master_key {
        return Err(RpcError::new(
            "config.invalid",
            "rotateMasterKey is not supported in MVP".to_string(),
        ));
    }

    ensure_master_key_present()?;
    Ok(RpcSettings::from(settings))
}

#[derive(Debug, serde::Serialize)]
struct TelegramValidateResponse {
    bot_username: String,
    chat_id: String,
}

#[tauri::command]
async fn telegram_validate(app: tauri::AppHandle) -> Result<TelegramValidateResponse, RpcError> {
    let settings = app_config::load_settings(&app)?;
    let token = secrets::get_secret(&settings.telegram.bot_token_key)?
        .ok_or_else(|| RpcError::new("telegram.unauthorized", "Bot token missing".to_string()))?;
    if settings.telegram.chat_id.is_empty() {
        return Err(RpcError::new(
            "config.invalid",
            "telegram.chat_id is empty".to_string(),
        ));
    }

    let client = reqwest::Client::new();
    let base = format!("https://api.telegram.org/bot{token}");

    #[derive(serde::Deserialize)]
    struct GetMeOk {
        ok: bool,
        result: GetMeResult,
    }
    #[derive(serde::Deserialize)]
    struct GetMeResult {
        username: Option<String>,
    }

    let me: GetMeOk = client
        .get(format!("{base}/getMe"))
        .send()
        .await
        .map_err(|e| RpcError::new("telegram.unavailable", format!("getMe failed: {e}")))?
        .json()
        .await
        .map_err(|e| RpcError::new("telegram.unavailable", format!("getMe json failed: {e}")))?;

    if !me.ok {
        return Err(RpcError::new(
            "telegram.unauthorized",
            "getMe returned ok=false".to_string(),
        ));
    }

    // Validate chat_id by calling getChat.
    #[derive(serde::Deserialize)]
    struct GetChatOk {
        ok: bool,
        description: Option<String>,
    }
    let chat: GetChatOk = client
        .get(format!("{base}/getChat"))
        .query(&[("chat_id", settings.telegram.chat_id.clone())])
        .send()
        .await
        .map_err(|e| RpcError::new("telegram.unavailable", format!("getChat failed: {e}")))?
        .json()
        .await
        .map_err(|e| RpcError::new("telegram.unavailable", format!("getChat json failed: {e}")))?;
    if !chat.ok {
        return Err(RpcError::new(
            "telegram.chat_not_found",
            chat.description
                .unwrap_or_else(|| "getChat ok=false".to_string()),
        ));
    }

    Ok(TelegramValidateResponse {
        bot_username: me
            .result
            .username
            .unwrap_or_else(|| "<unknown>".to_string()),
        chat_id: settings.telegram.chat_id,
    })
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct BackupStartRequest {
    source_path: String,
    label: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupStartResponse {
    task_id: String,
    snapshot_id: String,
}

#[tauri::command]
async fn backup_start(
    app: tauri::AppHandle,
    tasks: tauri::State<'_, Arc<TaskManager>>,
    req: BackupStartRequest,
) -> Result<BackupStartResponse, RpcError> {
    tasks.ensure_no_running("backup").await?;

    let settings = app_config::load_settings(&app)?;
    let token = secrets::get_secret(&settings.telegram.bot_token_key)?
        .ok_or_else(|| RpcError::new("keychain.unavailable", "Bot token missing".to_string()))?;
    let master_key = load_master_key()?;

    let db_path = app_config::index_db_path(&app)?;
    let storage = televy_backup_core::TelegramBotApiStorage::new(
        televy_backup_core::TelegramBotApiStorageConfig {
            bot_token: token,
            chat_id: settings.telegram.chat_id.clone(),
        },
    );

    let (task_id, cancel) = tasks.start_task(&app, "backup", "scan").await;
    tasks.set_running(&app, &task_id).await;

    let snapshot_id = format!("snp_{}", uuid::Uuid::new_v4());
    tasks.set_snapshot_id(&task_id, &snapshot_id).await;

    let app2 = app.clone();
    let tasks2 = tasks.inner().clone();
    let task_id2 = task_id.clone();
    let snapshot_id2 = snapshot_id.clone();
    let req_source = req.source_path.clone();
    let req_label = req.label.clone();
    tokio::spawn(async move {
        let sink = TauriProgressSink::new(app2.clone(), tasks2.clone(), task_id2.clone());
        let cfg = televy_backup_core::BackupConfig {
            db_path,
            source_path: std::path::PathBuf::from(req_source),
            label: req_label,
            chunking: televy_backup_core::ChunkingConfig {
                min_bytes: settings.chunking.min_bytes,
                avg_bytes: settings.chunking.avg_bytes,
                max_bytes: settings.chunking.max_bytes,
            },
            master_key,
            snapshot_id: Some(snapshot_id2.clone()),
            keep_last_snapshots: settings.retention.keep_last_snapshots,
        };

        let res = televy_backup_core::run_backup_with(
            &storage,
            cfg,
            televy_backup_core::BackupOptions {
                cancel: Some(&cancel),
                progress: Some(&sink),
            },
        )
        .await;

        match res {
            Ok(_) => {
                tasks2.finish_ok(&app2, &task_id2).await;
            }
            Err(e) => {
                let err = map_core_error(e);
                tasks2.finish_err(&app2, &task_id2, &err).await;
            }
        }
    });

    Ok(BackupStartResponse {
        task_id,
        snapshot_id,
    })
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TaskStatusResponse {
    state: String,
    phase: String,
    progress: serde_json::Value,
}

#[tauri::command]
async fn backup_status(
    tasks: tauri::State<'_, Arc<TaskManager>>,
    task_id: String,
) -> Result<TaskStatusResponse, RpcError> {
    let (state, phase, progress) = tasks
        .status(&task_id)
        .await
        .ok_or_else(|| RpcError::new("task.not_found", "task not found".to_string()))?;

    Ok(TaskStatusResponse {
        state,
        phase,
        progress: serde_json::to_value(progress).unwrap_or_else(|_| serde_json::json!({})),
    })
}

#[tauri::command]
async fn backup_cancel(
    tasks: tauri::State<'_, Arc<TaskManager>>,
    task_id: String,
) -> Result<bool, RpcError> {
    let ok = tasks.cancel(&task_id).await;
    if !ok {
        return Err(RpcError::new(
            "task.not_found",
            "task not found".to_string(),
        ));
    }
    Ok(true)
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RestoreStartRequest {
    snapshot_id: String,
    target_path: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct RestoreStartResponse {
    task_id: String,
}

#[tauri::command]
async fn restore_start(
    app: tauri::AppHandle,
    tasks: tauri::State<'_, Arc<TaskManager>>,
    req: RestoreStartRequest,
) -> Result<RestoreStartResponse, RpcError> {
    tasks.ensure_no_running("restore").await?;

    let settings = app_config::load_settings(&app)?;
    let token = secrets::get_secret(&settings.telegram.bot_token_key)?
        .ok_or_else(|| RpcError::new("keychain.unavailable", "Bot token missing".to_string()))?;
    let master_key = load_master_key()?;
    let db_path = app_config::index_db_path(&app)?;

    let storage = televy_backup_core::TelegramBotApiStorage::new(
        televy_backup_core::TelegramBotApiStorageConfig {
            bot_token: token,
            chat_id: settings.telegram.chat_id.clone(),
        },
    );

    let manifest_object_id = find_manifest_object_id(&db_path, &req.snapshot_id).await?;

    let (task_id, cancel) = tasks.start_task(&app, "restore", "index").await;
    tasks.set_running(&app, &task_id).await;

    let app2 = app.clone();
    let tasks2 = tasks.inner().clone();
    let task_id2 = task_id.clone();
    let snapshot_id = req.snapshot_id.clone();
    let target_path = req.target_path.clone();
    tokio::spawn(async move {
        let sink = TauriProgressSink::new(app2.clone(), tasks2.clone(), task_id2.clone());
        let res = televy_backup_core::restore_snapshot_with(
            &storage,
            televy_backup_core::RestoreConfig {
                snapshot_id: snapshot_id.clone(),
                manifest_object_id,
                master_key,
                index_db_path: db_path.clone(),
                target_path: std::path::PathBuf::from(target_path),
            },
            televy_backup_core::RestoreOptions {
                cancel: Some(&cancel),
                progress: Some(&sink),
            },
        )
        .await;
        match res {
            Ok(_) => {
                tasks2.finish_ok(&app2, &task_id2).await;
            }
            Err(e) => {
                let err = map_core_error(e);
                tasks2.finish_err(&app2, &task_id2, &err).await;
            }
        }
    });

    Ok(RestoreStartResponse { task_id })
}

#[tauri::command]
async fn restore_status(
    tasks: tauri::State<'_, Arc<TaskManager>>,
    task_id: String,
) -> Result<TaskStatusResponse, RpcError> {
    backup_status(tasks, task_id).await
}

#[tauri::command]
async fn restore_cancel(
    tasks: tauri::State<'_, Arc<TaskManager>>,
    task_id: String,
) -> Result<bool, RpcError> {
    backup_cancel(tasks, task_id).await
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct VerifyStartRequest {
    snapshot_id: String,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VerifyStartResponse {
    task_id: String,
}

#[tauri::command]
async fn verify_start(
    app: tauri::AppHandle,
    tasks: tauri::State<'_, Arc<TaskManager>>,
    req: VerifyStartRequest,
) -> Result<VerifyStartResponse, RpcError> {
    tasks.ensure_no_running("verify").await?;

    let settings = app_config::load_settings(&app)?;
    let token = secrets::get_secret(&settings.telegram.bot_token_key)?
        .ok_or_else(|| RpcError::new("keychain.unavailable", "Bot token missing".to_string()))?;
    let master_key = load_master_key()?;
    let db_path = app_config::index_db_path(&app)?;

    let storage = televy_backup_core::TelegramBotApiStorage::new(
        televy_backup_core::TelegramBotApiStorageConfig {
            bot_token: token,
            chat_id: settings.telegram.chat_id.clone(),
        },
    );

    let manifest_object_id = find_manifest_object_id(&db_path, &req.snapshot_id).await?;

    let (task_id, cancel) = tasks.start_task(&app, "verify", "index").await;
    tasks.set_running(&app, &task_id).await;

    let app2 = app.clone();
    let tasks2 = tasks.inner().clone();
    let task_id2 = task_id.clone();
    let snapshot_id = req.snapshot_id.clone();
    tokio::spawn(async move {
        let sink = TauriProgressSink::new(app2.clone(), tasks2.clone(), task_id2.clone());
        let res = televy_backup_core::verify_snapshot_with(
            &storage,
            televy_backup_core::VerifyConfig {
                snapshot_id: snapshot_id.clone(),
                manifest_object_id,
                master_key,
                index_db_path: db_path.clone(),
            },
            televy_backup_core::VerifyOptions {
                cancel: Some(&cancel),
                progress: Some(&sink),
            },
        )
        .await;
        match res {
            Ok(_) => {
                tasks2.finish_ok(&app2, &task_id2).await;
            }
            Err(e) => {
                let err = map_core_error(e);
                tasks2.finish_err(&app2, &task_id2, &err).await;
            }
        }
    });

    Ok(VerifyStartResponse { task_id })
}

#[tauri::command]
async fn verify_status(
    tasks: tauri::State<'_, Arc<TaskManager>>,
    task_id: String,
) -> Result<TaskStatusResponse, RpcError> {
    backup_status(tasks, task_id).await
}

#[tauri::command]
async fn backup_list_snapshots(app: tauri::AppHandle) -> Result<Vec<String>, RpcError> {
    let db_path = app_config::index_db_path(&app)?;
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .map_err(|e| RpcError::new("db.unavailable", format!("open db failed: {e}")))?;

    let rows = sqlx::query("SELECT snapshot_id FROM snapshots ORDER BY created_at DESC LIMIT 50")
        .fetch_all(&pool)
        .await
        .map_err(|e| RpcError::new("db.unavailable", format!("query failed: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|r| r.get::<String, _>("snapshot_id"))
        .collect())
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct StatsGetResponse {
    snapshots_total: i64,
    chunks_total: i64,
    bytes_uploaded_total: i64,
    bytes_deduped_total: i64,
}

#[tauri::command]
async fn stats_get(app: tauri::AppHandle) -> Result<StatsGetResponse, RpcError> {
    let db_path = app_config::index_db_path(&app)?;
    if !db_path.exists() {
        return Ok(StatsGetResponse {
            snapshots_total: 0,
            chunks_total: 0,
            bytes_uploaded_total: 0,
            bytes_deduped_total: 0,
        });
    }

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .map_err(|e| RpcError::new("db.unavailable", format!("open db failed: {e}")))?;

    let snapshots_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM snapshots")
        .fetch_one(&pool)
        .await
        .map_err(|e| RpcError::new("db.unavailable", format!("query failed: {e}")))?;

    let chunks_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM chunks")
        .fetch_one(&pool)
        .await
        .map_err(|e| RpcError::new("db.unavailable", format!("query failed: {e}")))?;

    let bytes_uploaded_total: i64 = sqlx::query_scalar("SELECT COALESCE(SUM(size), 0) FROM chunks")
        .fetch_one(&pool)
        .await
        .map_err(|e| RpcError::new("db.unavailable", format!("query failed: {e}")))?;

    Ok(StatsGetResponse {
        snapshots_total,
        chunks_total,
        bytes_uploaded_total,
        bytes_deduped_total: 0,
    })
}

#[derive(Clone)]
struct TauriProgressSink {
    app: tauri::AppHandle,
    tasks: Arc<TaskManager>,
    task_id: String,
}

impl TauriProgressSink {
    fn new(app: tauri::AppHandle, tasks: Arc<TaskManager>, task_id: String) -> Self {
        Self {
            app,
            tasks,
            task_id,
        }
    }
}

impl televy_backup_core::ProgressSink for TauriProgressSink {
    fn on_progress(&self, progress: televy_backup_core::TaskProgress) {
        let payload = TaskProgressEvent {
            task_id: self.task_id.clone(),
            phase: progress.phase,
            files_total: progress.files_total,
            files_done: progress.files_done,
            chunks_total: progress.chunks_total,
            chunks_done: progress.chunks_done,
            bytes_read: progress.bytes_read,
            bytes_uploaded: progress.bytes_uploaded,
            bytes_deduped: progress.bytes_deduped,
        };

        let app = self.app.clone();
        let tasks = self.tasks.clone();
        let task_id = self.task_id.clone();
        tokio::spawn(async move {
            tasks.update_progress(&app, &task_id, payload).await;
        });
    }
}

fn ensure_master_key_present() -> Result<(), RpcError> {
    if secrets::get_secret(MASTER_KEY_KEY)?.is_some() {
        return Ok(());
    }

    let mut key = [0u8; 32];
    getrandom::getrandom(&mut key)
        .map_err(|e| RpcError::new("keychain.write_failed", format!("random failed: {e}")))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(key);
    secrets::set_secret(MASTER_KEY_KEY, &b64)?;
    Ok(())
}

fn load_master_key() -> Result<[u8; 32], RpcError> {
    ensure_master_key_present()?;
    let b64 = secrets::get_secret(MASTER_KEY_KEY)?
        .ok_or_else(|| RpcError::new("keychain.unavailable", "master key missing".to_string()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| {
            RpcError::new(
                "keychain.unavailable",
                format!("master key decode failed: {e}"),
            )
        })?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| {
        RpcError::new(
            "keychain.unavailable",
            "master key invalid length".to_string(),
        )
    })?;
    Ok(arr)
}

fn map_core_error(e: televy_backup_core::Error) -> RpcError {
    match e {
        televy_backup_core::Error::Cancelled => {
            RpcError::new("task.cancelled", "Cancelled".to_string())
        }
        televy_backup_core::Error::InvalidConfig { message } => {
            RpcError::new("config.invalid", message)
        }
        televy_backup_core::Error::Telegram { message } => {
            RpcError::retryable("telegram.unavailable", message)
        }
        other => RpcError::new("internal", other.to_string()),
    }
}

async fn find_manifest_object_id(
    db_path: &std::path::Path,
    snapshot_id: &str,
) -> Result<String, RpcError> {
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .map_err(|e| RpcError::new("db.unavailable", format!("open db failed: {e}")))?;

    let row =
        sqlx::query("SELECT manifest_object_id FROM remote_indexes WHERE snapshot_id = ? LIMIT 1")
            .bind(snapshot_id)
            .fetch_optional(&pool)
            .await
            .map_err(|e| RpcError::new("db.unavailable", format!("query failed: {e}")))?;

    row.map(|r| r.get::<String, _>("manifest_object_id"))
        .ok_or_else(|| RpcError::new("snapshot.not_found", "snapshot not found".to_string()))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(Arc::new(TaskManager::default()))
        .plugin(tauri_plugin_store::Builder::default().build())
        .invoke_handler(tauri::generate_handler![
            ping,
            settings_get,
            settings_set,
            telegram_validate,
            backup_start,
            backup_status,
            backup_cancel,
            restore_start,
            restore_status,
            restore_cancel,
            verify_start,
            verify_status,
            backup_list_snapshots,
            stats_get
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
