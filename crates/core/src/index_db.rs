use std::path::Path;
use std::time::Duration;

use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};
use tracing::{debug, error};

use crate::Result;

// Large endpoint index DBs can legitimately take a long time to open (e.g. journal recovery after
// crashes or forced termination). Keep the pool acquire timeout comfortably above the default so
// backups don't fail with `pool timed out` while the DB is still doing valid work.
const SQLITE_POOL_ACQUIRE_TIMEOUT: Duration = Duration::from_secs(10 * 60);

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
        .acquire_timeout(SQLITE_POOL_ACQUIRE_TIMEOUT)
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
    sqlx::query("PRAGMA busy_timeout = 60000;")
        .execute(&pool)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.pragma_failed",
                db_path = %path.display(),
                pragma = "busy_timeout",
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
        .acquire_timeout(SQLITE_POOL_ACQUIRE_TIMEOUT)
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
    sqlx::query("PRAGMA busy_timeout = 60000;")
        .execute(&pool)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.pragma_failed",
                db_path = %path.display(),
                pragma = "busy_timeout",
                error = %e,
                "io.sqlite.pragma_failed"
            );
            e
        })?;

    Ok(pool)
}
