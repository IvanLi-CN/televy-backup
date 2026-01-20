use std::path::PathBuf;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid config: {message}")]
    InvalidConfig { message: String },

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite error: {0}")]
    Sqlite(#[from] sqlx::Error),

    #[error("sqlite migrate error: {0}")]
    SqliteMigrate(#[from] sqlx::migrate::MigrateError),

    #[error("walkdir error: {0}")]
    Walkdir(#[from] walkdir::Error),

    #[error("crypto error")]
    Crypto,

    #[error("cancelled")]
    Cancelled,

    #[error("telegram bot api error: {message}")]
    Telegram { message: String },

    #[error("missing index part: snapshot_id={snapshot_id} part_no={part_no}")]
    MissingIndexPart { snapshot_id: String, part_no: u32 },

    #[error("missing chunk object: chunk_hash={chunk_hash}")]
    MissingChunkObject { chunk_hash: String },

    #[error("integrity check failed: {message}")]
    Integrity { message: String },

    #[error("unsupported path (must be UTF-8): {path:?}")]
    NonUtf8Path { path: PathBuf },
}
