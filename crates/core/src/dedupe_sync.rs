use std::path::Path;

use sqlx::Connection;
use tracing::debug;

use crate::dedupe_catalog::{DedupeCatalogV1, load_remote_dedupe_catalog};
use crate::index_db::open_index_db;
use crate::index_sync::{
    ENDPOINT_STATE_DEDUPE_CATALOG_OBJECT_ID_KEY, ENDPOINT_STATE_ENDPOINT_DEDUPE_ID_KEY,
    endpoint_state_get,
};
use crate::progress::ProgressSink;
use crate::remote_index_db::download_and_write_index_db_atomic;
use crate::storage::Storage;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct DedupeMaterializeStats {
    pub base_bytes_downloaded: u64,
    pub delta_bytes_downloaded: u64,
}

pub async fn local_dedupe_db_matches_remote_latest(
    db_path: &Path,
    catalog_object_id: &str,
) -> Result<bool> {
    let Some(v) = endpoint_state_get(db_path, ENDPOINT_STATE_DEDUPE_CATALOG_OBJECT_ID_KEY).await?
    else {
        return Ok(false);
    };
    Ok(v == catalog_object_id)
}

pub async fn materialize_remote_dedupe_db<S: Storage>(
    storage: &S,
    master_key: &[u8; 32],
    endpoint_dedupe_id: &str,
    catalog_object_id: &str,
    out_path: &Path,
    normalize_provider: Option<&str>,
    progress: Option<&dyn ProgressSink>,
) -> Result<DedupeMaterializeStats> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp_parent = out_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let temp_dir = tempfile::Builder::new()
        .prefix("televy-dedupe-sync-")
        .tempdir_in(&temp_parent)?;

    let cat: DedupeCatalogV1 =
        load_remote_dedupe_catalog(storage, master_key, catalog_object_id).await?;
    if cat.endpoint_dedupe_id != endpoint_dedupe_id {
        return Err(Error::InvalidConfig {
            message: format!(
                "dedupe catalog endpoint mismatch: expected={} got={}",
                endpoint_dedupe_id, cat.endpoint_dedupe_id
            ),
        });
    }

    let base_path = temp_dir.path().join("dedupe.base.sqlite");
    let base_stats = download_and_write_index_db_atomic(
        storage,
        &cat.base.base_id,
        &cat.base.manifest_object_id,
        master_key,
        &base_path,
        None,
        normalize_provider,
        progress,
    )
    .await?;

    // Ensure migrations (e.g. endpoint_state) exist before we apply deltas and store state.
    let base_pool = open_index_db(&base_path).await?;
    let mut base_conn = base_pool.acquire().await?;
    drop(base_pool);

    let mut delta_bytes_downloaded = 0u64;
    for (idx, delta) in cat.deltas.iter().enumerate() {
        let delta_path = temp_dir.path().join(format!("dedupe.delta.{idx}.sqlite"));
        let stats = download_and_write_index_db_atomic(
            storage,
            &delta.delta_id,
            &delta.manifest_object_id,
            master_key,
            &delta_path,
            None,
            normalize_provider,
            progress,
        )
        .await?;
        delta_bytes_downloaded = delta_bytes_downloaded.saturating_add(stats.bytes_downloaded);

        apply_dedupe_delta(&mut base_conn, &delta_path).await?;
        let _ = std::fs::remove_file(&delta_path);
    }

    // Record the materialized state so future runs can skip redundant rebuilds.
    sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
        .bind(ENDPOINT_STATE_ENDPOINT_DEDUPE_ID_KEY)
        .bind(endpoint_dedupe_id)
        .execute(&mut *base_conn)
        .await?;
    sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
        .bind(ENDPOINT_STATE_DEDUPE_CATALOG_OBJECT_ID_KEY)
        .bind(catalog_object_id)
        .execute(&mut *base_conn)
        .await?;

    drop(base_conn);

    // Atomic publish of the materialized DB.
    replace_atomic(&base_path, out_path)?;

    debug!(
        event = "dedupe.sync.finish",
        endpoint_dedupe_id,
        catalog_object_id,
        base_bytes_downloaded = base_stats.bytes_downloaded,
        delta_bytes_downloaded,
        "dedupe.sync.finish"
    );

    Ok(DedupeMaterializeStats {
        base_bytes_downloaded: base_stats.bytes_downloaded,
        delta_bytes_downloaded,
    })
}

async fn apply_dedupe_delta(
    base_conn: &mut sqlx::SqliteConnection,
    delta_path: &Path,
) -> Result<()> {
    // ATTACH needs a literal string; escape single quotes defensively.
    let src_path_sql = delta_path.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("ATTACH DATABASE '{src_path_sql}' AS src"))
        .execute(&mut *base_conn)
        .await?;

    let mut tx = base_conn.begin().await?;

    sqlx::query(
        r#"
        INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
        SELECT chunk_hash, size, hash_alg, enc_alg, created_at
        FROM src.chunks
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT OR REPLACE INTO chunk_objects (chunk_hash, provider, object_id, created_at)
        SELECT chunk_hash, provider, object_id, created_at
        FROM src.chunk_objects
        "#,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    sqlx::query("DETACH DATABASE src")
        .execute(&mut *base_conn)
        .await?;

    Ok(())
}

fn replace_atomic(tmp: &Path, path: &Path) -> std::io::Result<()> {
    match std::fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let mut backup = path.to_path_buf();
            backup.set_extension(format!("bak-{}", uuid::Uuid::new_v4()));

            std::fs::rename(path, &backup)?;
            match std::fs::rename(tmp, path) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&backup);
                    Ok(())
                }
                Err(e) => {
                    let _ = std::fs::rename(&backup, path);
                    let _ = std::fs::remove_file(tmp);
                    Err(e)
                }
            }
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;

    #[tokio::test]
    async fn local_match_missing_file_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedupe.sqlite");
        let ok = local_dedupe_db_matches_remote_latest(&path, "obj")
            .await
            .unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn local_match_detects_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dedupe.sqlite");

        // Create DB and set state.
        let pool = open_index_db(&path).await.unwrap();
        sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
            .bind(ENDPOINT_STATE_DEDUPE_CATALOG_OBJECT_ID_KEY)
            .bind("cat_1")
            .execute(&pool)
            .await
            .unwrap();

        let ok = local_dedupe_db_matches_remote_latest(&path, "cat_1")
            .await
            .unwrap();
        assert!(ok);
    }

    #[tokio::test]
    async fn apply_delta_updates_object_id_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let base_path = dir.path().join("base.sqlite");
        let delta_path = dir.path().join("delta.sqlite");

        let base_pool = open_index_db(&base_path).await.unwrap();
        sqlx::query(
            "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES ('h', 1, 'blake3', 'xchacha20poly1305', '2026-03-02T00:00:00Z')",
        )
        .execute(&base_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at) VALUES ('h', 'p', 'old', '2026-03-02T00:00:00Z')",
        )
        .execute(&base_pool)
        .await
        .unwrap();
        drop(base_pool);

        let delta_pool = open_index_db(&delta_path).await.unwrap();
        sqlx::query(
            "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES ('h', 1, 'blake3', 'xchacha20poly1305', '2026-03-02T00:00:00Z')",
        )
        .execute(&delta_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at) VALUES ('h', 'p', 'new', '2026-03-02T00:00:01Z')",
        )
        .execute(&delta_pool)
        .await
        .unwrap();
        drop(delta_pool);

        let pool = open_index_db(&base_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        drop(pool);

        apply_dedupe_delta(&mut conn, &delta_path).await.unwrap();
        apply_dedupe_delta(&mut conn, &delta_path).await.unwrap();

        let row = sqlx::query(
            "SELECT object_id FROM chunk_objects WHERE provider = 'p' AND chunk_hash = 'h' LIMIT 1",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        let obj: String = row.get("object_id");
        assert_eq!(obj, "new");
    }
}
