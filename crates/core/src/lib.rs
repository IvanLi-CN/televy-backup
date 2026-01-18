mod backup;
mod crypto;
mod error;
mod index_db;
mod storage;

pub const APP_NAME: &str = "TelevyBackup";

pub use backup::{BackupConfig, BackupResult, ChunkingConfig, run_backup};
pub use error::{Error, Result};
pub use storage::{InMemoryStorage, Storage, TelegramBotApiStorage, TelegramBotApiStorageConfig};
