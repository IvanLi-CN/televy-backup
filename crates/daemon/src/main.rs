use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use base64::Engine;
use chrono::{Datelike, Timelike};
use serde::Deserialize;
use televy_backup_core::{
    BackupConfig, BackupOptions, ChunkingConfig, TelegramBotApiStorage, TelegramBotApiStorageConfig,
};
use tokio::time::{Duration, sleep};

#[derive(Debug, Clone, Deserialize)]
struct Settings {
    sources: Vec<String>,
    schedule: Schedule,
    retention: Retention,
    chunking: Chunking,
    telegram: Telegram,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct Schedule {
    enabled: bool,
    kind: String,
    hourly_minute: u8,
    daily_at: String,
    timezone: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Retention {
    keep_last_snapshots: u32,
}

#[derive(Debug, Clone, Deserialize)]
struct Chunking {
    min_bytes: u32,
    avg_bytes: u32,
    max_bytes: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct Telegram {
    mode: String,
    chat_id: String,
    bot_token_key: String,
    rate_limit: TelegramRateLimit,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct TelegramRateLimit {
    max_concurrent_uploads: u32,
    min_delay_ms: u32,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = std::env::var("TELEVYBACKUP_CONFIG_DIR")
        .ok()
        .map(PathBuf::from);
    let data_dir = std::env::var("TELEVYBACKUP_DATA_DIR")
        .ok()
        .map(PathBuf::from);

    let config_path = config_dir
        .unwrap_or_else(default_config_dir)
        .join("config.toml");
    let db_path = data_dir
        .unwrap_or_else(default_data_dir)
        .join("index")
        .join("index.sqlite");

    let settings = load_settings(&config_path)?;
    if settings.telegram.mode != "botapi" {
        return Err("telegram.mode must be botapi".into());
    }

    let bot_token =
        get_secret(&settings.telegram.bot_token_key)?.ok_or("telegram bot token missing")?;
    let master_key = load_master_key()?;

    let storage = TelegramBotApiStorage::new(TelegramBotApiStorageConfig {
        bot_token,
        chat_id: settings.telegram.chat_id.clone(),
    });

    let mut last_hourly: Option<(i32, u32, u32, u32)> = None; // year, month, day, hour
    let mut last_daily: Option<(i32, u32, u32)> = None; // year, month, day

    loop {
        if settings.schedule.enabled {
            let now = chrono::Local::now();
            let should_run = match settings.schedule.kind.as_str() {
                "hourly" => {
                    let key = (now.year(), now.month(), now.day(), now.hour());
                    now.minute() as u8 == settings.schedule.hourly_minute
                        && last_hourly != Some(key)
                }
                "daily" => {
                    let key = (now.year(), now.month(), now.day());
                    let (hh, mm) = parse_hhmm(&settings.schedule.daily_at)?;
                    now.hour() as u8 == hh && now.minute() as u8 == mm && last_daily != Some(key)
                }
                other => return Err(format!("unknown schedule.kind: {other}").into()),
            };

            if should_run {
                match settings.schedule.kind.as_str() {
                    "hourly" => {
                        last_hourly = Some((now.year(), now.month(), now.day(), now.hour()));
                    }
                    "daily" => {
                        last_daily = Some((now.year(), now.month(), now.day()));
                    }
                    _ => {}
                }

                for source in &settings.sources {
                    let cfg = BackupConfig {
                        db_path: db_path.clone(),
                        source_path: PathBuf::from(source),
                        label: "scheduled".to_string(),
                        chunking: ChunkingConfig {
                            min_bytes: settings.chunking.min_bytes,
                            avg_bytes: settings.chunking.avg_bytes,
                            max_bytes: settings.chunking.max_bytes,
                        },
                        master_key,
                        snapshot_id: None,
                        keep_last_snapshots: settings.retention.keep_last_snapshots,
                    };

                    let _ = televy_backup_core::run_backup_with(
                        &storage,
                        cfg,
                        BackupOptions::default(),
                    )
                    .await;
                }
            }
        }

        sleep(Duration::from_secs(30)).await;
    }
}

fn load_settings(path: &Path) -> Result<Settings, Box<dyn std::error::Error>> {
    let text = std::fs::read_to_string(path)?;
    let s: Settings = toml::from_str(&text)?;
    Ok(s)
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

#[cfg(target_os = "macos")]
fn get_secret(key: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let entry = keyring::Entry::new("TelevyBackup", key)?;
    match entry.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(not(target_os = "macos"))]
fn get_secret(_key: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Ok(None)
}

#[cfg(target_os = "macos")]
fn load_master_key() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let b64 = get_secret("televybackup.master_key")?.ok_or("master key missing")?;
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| "invalid master key length")?;
    Ok(arr)
}

#[cfg(not(target_os = "macos"))]
fn load_master_key() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    Err("master key only supported on macOS in this build".into())
}
