use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use base64::Engine;
use chrono::{Datelike, Timelike};
use sqlx::Row;
use televy_backup_core::{
    BackupConfig, BackupOptions, ChunkingConfig, TelegramMtProtoStorage, TelegramMtProtoStorageConfig,
};
use televy_backup_core::Storage;
use televy_backup_core::{bootstrap, config as settings_config};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

#[derive(Default, Clone)]
struct TargetScheduleState {
    last_hourly: Option<(i32, u32, u32, u32)>, // year, month, day, hour
    last_daily: Option<(i32, u32, u32)>,       // year, month, day
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = std::env::var("TELEVYBACKUP_CONFIG_DIR")
        .ok()
        .map(PathBuf::from);
    let data_dir = std::env::var("TELEVYBACKUP_DATA_DIR")
        .ok()
        .map(PathBuf::from);

    let config_root = config_dir.unwrap_or_else(default_config_dir);
    let data_root = data_dir.unwrap_or_else(default_data_dir);
    let db_path = data_root.join("index").join("index.sqlite");

    let settings = settings_config::load_settings_v2(&config_root)?;
    settings_config::validate_settings_schema_v2(&settings)?;

    if settings.telegram.mtproto.api_id <= 0 {
        return Err("telegram.mtproto.api_id must be > 0".into());
    }
    if settings.telegram.mtproto.api_hash_key.trim().is_empty() {
        return Err("telegram.mtproto.api_hash_key must not be empty".into());
    }

    let vault_key = load_or_create_vault_key()?;
    let secrets_path = televy_backup_core::secrets::secrets_path(&config_root);
    let mut secrets_store =
        televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)?;

    let master_key_b64 = get_secret_from_store(&secrets_store, MASTER_KEY_KEY)
        .or_else(|| keychain_get_secret(MASTER_KEY_KEY).ok().flatten())
        .ok_or("master key missing")?;
    let master_key = decode_base64_32(&master_key_b64)?;

    let api_hash = get_secret_from_store(&secrets_store, &settings.telegram.mtproto.api_hash_key)
        .ok_or("telegram api_hash missing")?;

    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut schedule_state_by_target = HashMap::<String, TargetScheduleState>::new();
    let mut storage_by_endpoint = HashMap::<String, TelegramMtProtoStorage>::new();

    loop {
        let now = chrono::Local::now();

        for target in &settings.targets {
            let eff = settings_config::effective_schedule(&settings.schedule, target.schedule.as_ref());
            if !target.enabled || !eff.enabled {
                continue;
            }

            let state = schedule_state_by_target
                .entry(target.id.clone())
                .or_default();

            let should_run = match eff.kind.as_str() {
                "hourly" => {
                    if now.minute() != eff.hourly_minute as u32 {
                        false
                    } else {
                        let key = (now.year(), now.month(), now.day(), now.hour());
                        if state.last_hourly == Some(key) {
                            false
                        } else {
                            state.last_hourly = Some(key);
                            true
                        }
                    }
                }
                "daily" => {
                    let (hh, mm) = parse_hhmm(&eff.daily_at)?;
                    if now.hour() != hh as u32 || now.minute() != mm as u32 {
                        false
                    } else {
                        let key = (now.year(), now.month(), now.day());
                        if state.last_daily == Some(key) {
                            false
                        } else {
                            state.last_daily = Some(key);
                            true
                        }
                    }
                }
                _ => false,
            };

            if !should_run {
                continue;
            }

            let Some(ep) = settings
                .telegram_endpoints
                .iter()
                .find(|e| e.id == target.endpoint_id)
            else {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    status = "failed",
                    error_code = "config.invalid",
                    error_message = "target references unknown endpoint_id",
                    target_id = %target.id,
                    endpoint_id = %target.endpoint_id,
                    "run.finish"
                );
                continue;
            };

            if ep.chat_id.trim().is_empty() {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    status = "failed",
                    error_code = "config.invalid",
                    error_message = "endpoint chat_id is empty",
                    target_id = %target.id,
                    endpoint_id = %ep.id,
                    "run.finish"
                );
                continue;
            }

            let bot_token = get_secret_from_store(&secrets_store, &ep.bot_token_key)
                .or_else(|| keychain_get_secret(&ep.bot_token_key).ok().flatten());
            let Some(bot_token) = bot_token else {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    status = "failed",
                    error_code = "telegram.unauthorized",
                    error_message = "bot token missing",
                    target_id = %target.id,
                    endpoint_id = %ep.id,
                    "run.finish"
                );
                continue;
            };

            if !storage_by_endpoint.contains_key(&ep.id) {
                let session = match get_secret_from_store(&secrets_store, &ep.mtproto.session_key) {
                    Some(b64) if !b64.trim().is_empty() => Some(
                        base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?,
                    ),
                    _ => None,
                };

                let cache_dir = data_root.join("cache").join("mtproto").join(&ep.id);
                std::fs::create_dir_all(&cache_dir)?;
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
                .await?;

                storage_by_endpoint.insert(ep.id.clone(), storage);
            }

            let storage = match storage_by_endpoint.get(&ep.id) {
                Some(s) => s,
                None => continue,
            };

            let task_id = format!("tsk_{}", Uuid::new_v4());
            let run_log = televy_backup_core::run_log::start_run_log("backup", &task_id, &data_root)?;

            tracing::info!(
                event = "run.start",
                kind = "backup",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %target.id,
                endpoint_id = %ep.id,
                source_path = %target.source_path,
                log_path = %run_log.path().display(),
                "run.start"
            );

            let started = Instant::now();
            let label = if target.label.trim().is_empty() {
                "scheduled".to_string()
            } else {
                target.label.clone()
            };

            let cfg = BackupConfig {
                db_path: db_path.clone(),
                source_path: PathBuf::from(&target.source_path),
                label: label.clone(),
                chunking: ChunkingConfig {
                    min_bytes: settings.chunking.min_bytes,
                    avg_bytes: settings.chunking.avg_bytes,
                    max_bytes: settings.chunking.max_bytes,
                },
                master_key,
                snapshot_id: None,
                keep_last_snapshots: settings.retention.keep_last_snapshots,
            };

            let result = televy_backup_core::run_backup_with(storage, cfg, BackupOptions::default()).await;
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

                    let pool = televy_backup_core::index_db::open_existing_index_db(&db_path).await?;
                    let row = sqlx::query(
                        "SELECT manifest_object_id FROM remote_indexes WHERE snapshot_id = ? AND provider = ? LIMIT 1",
                    )
                    .bind(&res.snapshot_id)
                    .bind(storage.provider())
                    .fetch_one(&pool)
                    .await?;
                    let manifest_object_id: String = row.get("manifest_object_id");

                    if let Err(e) = bootstrap::update_remote_latest(
                        storage,
                        &master_key,
                        &target.id,
                        &target.source_path,
                        &label,
                        &res.snapshot_id,
                        &manifest_object_id,
                    )
                    .await
                    {
                        tracing::error!(
                            event = "bootstrap.update_failed",
                            target_id = %target.id,
                            endpoint_id = %ep.id,
                            error = %e,
                            "bootstrap.update_failed"
                        );
                    }
                }
                Err(e) => {
                    tracing::error!(
                        event = "run.finish",
                        kind = "backup",
                        run_id = %task_id,
                        task_id = %task_id,
                        status = "failed",
                        duration_seconds,
                        error_code = e.code(),
                        error_message = %e,
                        "run.finish"
                    );
                }
            }

            if let Some(bytes) = storage.session_bytes() {
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                let should_write = secrets_store
                    .get(&ep.mtproto.session_key)
                    .is_none_or(|v| v != b64.as_str());
                if should_write {
                    secrets_store.set(&ep.mtproto.session_key, b64);
                    if let Err(e) = televy_backup_core::secrets::save_secrets_store(
                        &secrets_path,
                        &vault_key,
                        &secrets_store,
                    ) {
                        tracing::warn!(
                            event = "secrets.session_persist_failed",
                            error = %e,
                            "failed to persist mtproto session"
                        );
                    }
                }
            }
        }

        sleep(Duration::from_secs(30)).await;
    }
}

fn parse_hhmm(s: &str) -> Result<(u8, u8), Box<dyn std::error::Error>> {
    let (hh, mm) = s.split_once(':').ok_or("daily_at must be HH:MM")?;
    let hh: u8 = hh.parse()?;
    let mm: u8 = mm.parse()?;
    Ok((hh, mm))
}

fn default_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("TelevyBackup")
}

fn default_data_dir() -> PathBuf {
    default_config_dir()
}

const MASTER_KEY_KEY: &str = "televybackup.master_key";

fn get_secret_from_store(
    store: &televy_backup_core::secrets::SecretsStore,
    key: &str,
) -> Option<String> {
    store.get(key).map(|s| s.to_string())
}

fn decode_base64_32(b64: &str) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| "invalid key length")?;
    Ok(arr)
}

#[cfg(target_os = "macos")]
fn keychain_get_secret(key: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    use security_framework::passwords::{PasswordOptions, generic_password};

    let opts = PasswordOptions::new_generic_password(televy_backup_core::APP_NAME, key);
    match generic_password(opts) {
        Ok(bytes) => Ok(Some(String::from_utf8(bytes)?)),
        Err(e) => {
            if is_keychain_not_found(&e) {
                Ok(None)
            } else {
                Err(Box::new(e))
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
fn keychain_get_secret(_key: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Err("keychain only supported on macOS".into())
}

#[cfg(target_os = "macos")]
fn keychain_set_secret(key: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    use security_framework::passwords::set_generic_password;
    set_generic_password(televy_backup_core::APP_NAME, key, value.as_bytes())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn is_keychain_not_found(e: &security_framework::base::Error) -> bool {
    // errSecItemNotFound
    e.code() == -25300
}

#[cfg(target_os = "macos")]
fn load_or_create_vault_key() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let existing = keychain_get_secret(televy_backup_core::secrets::VAULT_KEY_KEY)?
        .map(|s| s.trim().to_string());

    if let Some(b64) = existing {
        return Ok(televy_backup_core::secrets::vault_key_from_base64(&b64)?);
    }

    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| std::io::Error::other(format!("getrandom failed: {e}")))?;
    let b64 = televy_backup_core::secrets::vault_key_to_base64(&bytes);
    keychain_set_secret(televy_backup_core::secrets::VAULT_KEY_KEY, &b64)?;
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
fn load_or_create_vault_key() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    Err("vault key only supported on macOS".into())
}
