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

    #[error("telegram bot api error: {message}")]
    Telegram { message: String },

    #[error("unsupported path (must be UTF-8): {path:?}")]
    NonUtf8Path { path: PathBuf },
}
