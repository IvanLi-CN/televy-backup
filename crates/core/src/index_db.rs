use std::path::Path;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use tracing::{debug, error};

use crate::Result;

pub async fn open_index_db(path: &Path) -> Result<SqlitePool> {
    debug!(
        event = "sqlite.open",
        db_path = %path.display(),
        create_if_missing = true,
        "sqlite.open"
    );
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Delete)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.connect_failed",
                db_path = %path.display(),
                error = %e,
                "io.sqlite.connect_failed"
            );
            e
        })?;

    sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&pool)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.pragma_failed",
                db_path = %path.display(),
                error = %e,
                "io.sqlite.pragma_failed"
            );
            e
        })?;

    sqlx::migrate!().run(&pool).await.map_err(|e| {
        error!(
            event = "io.sqlite.migrate_failed",
            db_path = %path.display(),
            error = %e,
            "io.sqlite.migrate_failed"
        );
        e
    })?;
    Ok(pool)
}

pub async fn open_existing_index_db(path: &Path) -> Result<SqlitePool> {
    debug!(
        event = "sqlite.open",
        db_path = %path.display(),
        create_if_missing = false,
        "sqlite.open"
    );
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(false)
        .journal_mode(SqliteJournalMode::Delete)
        .synchronous(SqliteSynchronous::Normal);

    let pool = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(options)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.connect_failed",
                db_path = %path.display(),
                error = %e,
                "io.sqlite.connect_failed"
            );
            e
        })?;

    sqlx::query("PRAGMA foreign_keys = ON;")
        .execute(&pool)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.pragma_failed",
                db_path = %path.display(),
                error = %e,
                "io.sqlite.pragma_failed"
            );
            e
        })?;

    Ok(pool)
}
