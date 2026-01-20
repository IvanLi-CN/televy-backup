mod backup;
mod crypto;
mod error;
pub mod index_db;
mod index_manifest;
mod pack;
mod progress;
mod restore;
mod storage;

pub const APP_NAME: &str = "TelevyBackup";

pub use backup::{
    BackupConfig, BackupOptions, BackupResult, ChunkingConfig, run_backup, run_backup_with,
};
pub use error::{Error, Result};
pub use progress::{ProgressSink, TaskProgress};
pub use restore::{
    RestoreConfig, RestoreOptions, RestoreResult, VerifyConfig, VerifyOptions, VerifyResult,
    restore_snapshot, restore_snapshot_with, verify_snapshot, verify_snapshot_with,
};
pub use storage::{
    ChunkObjectRef, InMemoryStorage, Storage, TelegramBotApiStorage, TelegramBotApiStorageConfig,
    encode_tgfile_object_id, encode_tgpack_object_id, parse_chunk_object_ref,
};
