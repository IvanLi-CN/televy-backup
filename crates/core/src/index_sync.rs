use std::path::Path;

use sqlx::Row;

pub const ENDPOINT_STATE_ENDPOINT_INDEX_ID_KEY: &str = "endpoint_index_id";
pub const ENDPOINT_STATE_ENDPOINT_MANIFEST_OBJECT_ID_KEY: &str = "endpoint_manifest_object_id";

pub async fn local_endpoint_db_matches_remote_latest(
    db_path: &Path,
    manifest_object_id: &str,
) -> crate::Result<bool> {
    let Some(v) =
        endpoint_state_get(db_path, ENDPOINT_STATE_ENDPOINT_MANIFEST_OBJECT_ID_KEY).await?
    else {
        return Ok(false);
    };
    Ok(v == manifest_object_id)
}

pub async fn endpoint_state_get(db_path: &Path, key: &str) -> crate::Result<Option<String>> {
    if !db_path.exists() {
        return Ok(None);
    }

    // Use `open_index_db` (not `open_existing_*`) so migrations (e.g. endpoint_state) are applied
    // even when the DB was downloaded from a remote older version.
    let pool = match crate::index_db::open_index_db(db_path).await {
        Ok(pool) => pool,
        Err(_) => return Ok(None),
    };

    let row = sqlx::query("SELECT value FROM endpoint_state WHERE key = ? LIMIT 1")
        .bind(key)
        .fetch_optional(&pool)
        .await?;

    Ok(row.map(|r| r.get::<String, _>("value")))
}

pub async fn endpoint_state_set(db_path: &Path, key: &str, value: &str) -> crate::Result<()> {
    // Ensure parent exists, and migrations are applied.
    let pool = crate::index_db::open_index_db(db_path).await?;
    sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
        .bind(key)
        .bind(value)
        .execute(&pool)
        .await?;
    Ok(())
}

pub async fn local_index_matches_remote_latest(
    db_path: &Path,
    provider: &str,
    snapshot_id: &str,
    manifest_object_id: &str,
) -> crate::Result<bool> {
    if !db_path.exists() {
        return Ok(false);
    }

    let pool = match crate::index_db::open_existing_index_db(db_path).await {
        Ok(pool) => pool,
        Err(_) => return Ok(false),
    };

    let row = match sqlx::query(
        "SELECT provider, manifest_object_id FROM remote_indexes WHERE snapshot_id = ? LIMIT 1",
    )
    .bind(snapshot_id)
    .fetch_optional(&pool)
    .await
    {
        Ok(row) => row,
        Err(_) => return Ok(false),
    };

    let Some(row) = row else {
        return Ok(false);
    };

    let row_provider: String = match row.try_get("provider") {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };
    let row_manifest_object_id: String = match row.try_get("manifest_object_id") {
        Ok(v) => v,
        Err(_) => return Ok(false),
    };

    if row_manifest_object_id != manifest_object_id {
        return Ok(false);
    }

    // Provider includes endpoint_id (e.g. `telegram.mtproto/default`). Across machines or configs,
    // endpoint IDs may differ even when using the same Telegram chat, so treat the "provider kind"
    // as authoritative for the "already synced" check.
    Ok(row_provider == provider || provider_kind(&row_provider) == provider_kind(provider))
}

fn provider_kind(provider: &str) -> &str {
    provider.split(['/', ':']).next().unwrap_or(provider).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn local_index_match_missing_file_is_false() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");
        let ok = local_index_matches_remote_latest(&path, "p", "s", "m")
            .await
            .unwrap();
        assert!(!ok);
    }

    #[tokio::test]
    async fn local_index_match_detects_match_and_stale() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("index.sqlite");

        let pool = crate::index_db::open_index_db(&path).await.unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, '2026-01-01T00:00:00Z', '/', 'manual', NULL)",
        )
        .bind("snp_1")
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO remote_indexes (snapshot_id, provider, manifest_object_id, created_at) VALUES (?, ?, ?, '2026-01-01T00:00:00Z')",
        )
        .bind("snp_1")
        .bind("telegram.mtproto:v1")
        .bind("obj_1")
        .execute(&pool)
        .await
        .unwrap();

        let ok = local_index_matches_remote_latest(&path, "telegram.mtproto:v1", "snp_1", "obj_1")
            .await
            .unwrap();
        assert!(ok);

        // Endpoint IDs differ across machines/configs; the provider kind should still match.
        let ok_other_provider = local_index_matches_remote_latest(
            &path,
            "telegram.mtproto/other-endpoint",
            "snp_1",
            "obj_1",
        )
        .await
        .unwrap();
        assert!(ok_other_provider);

        let stale =
            local_index_matches_remote_latest(&path, "telegram.mtproto:v1", "snp_1", "obj_2")
                .await
                .unwrap();
        assert!(!stale);
    }
}
