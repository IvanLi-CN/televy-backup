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

/// Opens the short-lived SQLite database assembled for one snapshot filemap.
///
/// The scan owns the only connection, so WAL avoids rollback-journal churn while
/// bounded multi-row statements run inside one scan transaction. Durability is
/// restored at the upload boundary before the WAL is checkpointed into the
/// uploaded main database.
pub async fn open_snapshot_filemap_db(path: &Path) -> Result<SqlitePool> {
    debug!(
        event = "sqlite.open",
        db_path = %path.display(),
        create_if_missing = true,
        role = "snapshot_filemap",
        "sqlite.open"
    );
    let options = SqliteConnectOptions::new()
        .filename(path)
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
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
    sqlx::query("PRAGMA cache_size = -65536;")
        .execute(&pool)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.pragma_failed",
                db_path = %path.display(),
                pragma = "cache_size",
                error = %e,
                "io.sqlite.pragma_failed"
            );
            e
        })?;
    sqlx::query("PRAGMA wal_autocheckpoint = 0;")
        .execute(&pool)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.pragma_failed",
                db_path = %path.display(),
                pragma = "wal_autocheckpoint",
                error = %e,
                "io.sqlite.pragma_failed"
            );
            e
        })?;
    sqlx::query("PRAGMA synchronous = OFF;")
        .execute(&pool)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.pragma_failed",
                db_path = %path.display(),
                pragma = "synchronous",
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
    // Snapshot filemaps contain one snapshot and are written once. The endpoint
    // query index is redundant here, while maintaining it for every file row
    // materially increases scan-time write cost. Restore and verify retain the
    // primary/unique indexes for their lookups and ordering.
    sqlx::query("DROP INDEX IF EXISTS idx_files_snapshot_kind_file")
        .execute(&pool)
        .await
        .map_err(|e| {
            error!(
                event = "io.sqlite.index_cleanup_failed",
                db_path = %path.display(),
                index = "idx_files_snapshot_kind_file",
                error = %e,
                "io.sqlite.index_cleanup_failed"
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

#[cfg(test)]
mod tests {
    use super::open_snapshot_filemap_db;

    #[tokio::test]
    async fn snapshot_filemap_defers_sync_and_auto_checkpoint() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("snapshot.sqlite");
        let pool = open_snapshot_filemap_db(&path).await.unwrap();

        let journal_mode: String = sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&pool)
            .await
            .unwrap();
        let synchronous: i64 = sqlx::query_scalar("PRAGMA synchronous")
            .fetch_one(&pool)
            .await
            .unwrap();
        let wal_autocheckpoint: i64 = sqlx::query_scalar("PRAGMA wal_autocheckpoint")
            .fetch_one(&pool)
            .await
            .unwrap();
        let cache_size: i64 = sqlx::query_scalar("PRAGMA cache_size")
            .fetch_one(&pool)
            .await
            .unwrap();
        let file_indexes: Vec<String> =
            sqlx::query_scalar("SELECT name FROM pragma_index_list('files') ORDER BY name")
                .fetch_all(&pool)
                .await
                .unwrap();

        assert_eq!(journal_mode, "wal");
        assert_eq!(synchronous, 0);
        assert_eq!(wal_autocheckpoint, 0);
        assert_eq!(cache_size, -65_536);
        assert!(
            !file_indexes
                .iter()
                .any(|name| name == "idx_files_snapshot_kind_file")
        );
    }
}
