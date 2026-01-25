mod backup;
pub mod bootstrap;
pub mod config;
mod crypto;
mod error;
pub mod gold_key;
pub mod index_db;
mod index_manifest;
mod pack;
mod progress;
mod restore;
pub mod status;
pub mod run_log;
pub mod secrets;
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
pub use status::{
    Counter, GlobalStatus, Progress, Rate, StatusSnapshot, StatusSource, TargetRunSummary,
    TargetState, now_unix_ms, read_status_snapshot_json, status_json_path,
    write_status_snapshot_json_atomic,
};
pub use storage::{
    ChunkObjectRef, InMemoryStorage, Storage, TelegramMtProtoStorage, TelegramMtProtoStorageConfig,
    TgMtProtoObjectIdV1, encode_tgfile_object_id, encode_tgmtproto_object_id_v1,
    encode_tgpack_object_id, parse_chunk_object_ref, parse_tgmtproto_object_id_v1,
};
