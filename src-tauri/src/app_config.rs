use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::rpc::RpcError;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub sources: Vec<String>,
    pub schedule: Schedule,
    pub retention: Retention,
    pub chunking: Chunking,
    pub telegram: Telegram,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub enabled: bool,
    pub kind: String, // "hourly" | "daily"
    pub hourly_minute: u8,
    pub daily_at: String, // "HH:MM"
    pub timezone: String, // "local"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retention {
    pub keep_last_snapshots: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunking {
    pub min_bytes: u32,
    pub avg_bytes: u32,
    pub max_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Telegram {
    pub mode: String, // "botapi"
    pub chat_id: String,
    pub bot_token_key: String,
    pub rate_limit: TelegramRateLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramRateLimit {
    pub max_concurrent_uploads: u32,
    pub min_delay_ms: u32,
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

pub const MASTER_KEY_KEY: &str = "televybackup.master_key";

pub fn config_dir(app: &tauri::AppHandle) -> Result<PathBuf, RpcError> {
    if let Ok(dir) = std::env::var("TELEVYBACKUP_CONFIG_DIR") {
        return Ok(PathBuf::from(dir));
    }
    app.path().app_config_dir().map_err(|e| {
        RpcError::new(
            "config.unavailable",
            format!("app config dir unavailable: {e}"),
        )
    })
}

pub fn data_dir(app: &tauri::AppHandle) -> Result<PathBuf, RpcError> {
    if let Ok(dir) = std::env::var("TELEVYBACKUP_DATA_DIR") {
        return Ok(PathBuf::from(dir));
    }
    app.path()
        .app_data_dir()
        .map_err(|e| RpcError::new("data.unavailable", format!("app data dir unavailable: {e}")))
}

pub fn config_path(app: &tauri::AppHandle) -> Result<PathBuf, RpcError> {
    Ok(config_dir(app)?.join("config.toml"))
}

pub fn index_db_path(app: &tauri::AppHandle) -> Result<PathBuf, RpcError> {
    Ok(data_dir(app)?.join("index").join("index.sqlite"))
}

pub fn load_settings(app: &tauri::AppHandle) -> Result<Settings, RpcError> {
    let path = config_path(app)?;
    if !path.exists() {
        return Ok(Settings::default());
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| RpcError::new("config.read_failed", format!("read config failed: {e}")))?;
    let s: Settings = toml::from_str(&text)
        .map_err(|e| RpcError::new("config.invalid", format!("parse config.toml failed: {e}")))?;
    Ok(s)
}

pub fn save_settings(app: &tauri::AppHandle, settings: &Settings) -> Result<(), RpcError> {
    validate_settings(settings)?;
    let path = config_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            RpcError::new(
                "config.write_failed",
                format!("create config dir failed: {e}"),
            )
        })?;
    }
    let text = toml::to_string(settings).map_err(|e| {
        RpcError::new(
            "config.invalid",
            format!("serialize config.toml failed: {e}"),
        )
    })?;
    atomic_write(&path, text.as_bytes())
        .map_err(|e| RpcError::new("config.write_failed", format!("write config failed: {e}")))?;
    Ok(())
}

fn validate_settings(settings: &Settings) -> Result<(), RpcError> {
    if settings.telegram.mode != "botapi" {
        return Err(RpcError::new(
            "config.invalid",
            "telegram.mode must be \"botapi\"".to_string(),
        ));
    }
    if settings.telegram.bot_token_key.is_empty() {
        return Err(RpcError::new(
            "config.invalid",
            "telegram.bot_token_key must not be empty".to_string(),
        ));
    }
    if settings.retention.keep_last_snapshots < 1 {
        return Err(RpcError::new(
            "config.invalid",
            "retention.keep_last_snapshots must be >= 1".to_string(),
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
