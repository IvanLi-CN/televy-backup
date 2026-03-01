use std::path::PathBuf;

use sqlx::Row;
use televy_backup_core::{
    BackupConfig, ChunkObjectRef, ChunkingConfig, InMemoryStorage, RestoreConfig, VerifyConfig,
    parse_chunk_object_ref, restore_snapshot, run_backup, verify_snapshot,
};
use tempfile::TempDir;

fn write_file(path: PathBuf, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn restore_and_verify_snapshot_from_remote_index() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    write_file(
        source.join("a.txt"),
        b"hello world\nhello world\nhello world\n",
    );
    write_file(source.join("nested/b.bin"), &[42u8; 10_000]);

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let master_key = [7u8; 32];

    let r1 = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path.clone(),
            filemap_dir: filemap_dir.clone(),
            source_path: source.clone(),
            label: "t1".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 64,
                avg_bytes: 256,
                max_bytes: 1024,
            },
            rate_limit: Default::default(),
            master_key,
            snapshot_id: None,
            keep_last_snapshots: 10,
        },
    )
    .await
    .unwrap();

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();

    let manifest_object_id: String =
        sqlx::query("SELECT manifest_object_id FROM remote_indexes WHERE snapshot_id = ? LIMIT 1")
            .bind(&r1.snapshot_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("manifest_object_id");

    let endpoint_manifest_object_id: String =
        sqlx::query("SELECT value FROM endpoint_state WHERE key = ? LIMIT 1")
            .bind(televy_backup_core::index_sync::ENDPOINT_STATE_ENDPOINT_MANIFEST_OBJECT_ID_KEY)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("value");

    let restore_filemap_db_path = temp.path().join("restored-filemap.sqlite");
    let restore_endpoint_db_path = temp.path().join("restored-endpoint.sqlite");
    let restore_target = temp.path().join("restored");

    restore_snapshot(
        &storage,
        RestoreConfig {
            snapshot_id: r1.snapshot_id.clone(),
            filemap_manifest_object_id: manifest_object_id.clone(),
            endpoint_manifest_object_id: Some(endpoint_manifest_object_id.clone()),
            endpoint_index_id: None,
            master_key,
            filemap_db_path: restore_filemap_db_path.clone(),
            endpoint_db_path: Some(restore_endpoint_db_path.clone()),
            target_path: restore_target.clone(),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read(source.join("a.txt")).unwrap(),
        std::fs::read(restore_target.join("a.txt")).unwrap()
    );
    assert_eq!(
        std::fs::read(source.join("nested/b.bin")).unwrap(),
        std::fs::read(restore_target.join("nested/b.bin")).unwrap()
    );

    let verify_index_db_path = temp.path().join("verify-index.sqlite");
    let vr = verify_snapshot(
        &storage,
        VerifyConfig {
            snapshot_id: r1.snapshot_id.clone(),
            filemap_manifest_object_id: manifest_object_id,
            endpoint_manifest_object_id: Some(endpoint_manifest_object_id),
            endpoint_index_id: None,
            master_key,
            filemap_db_path: verify_index_db_path,
            endpoint_db_path: Some(temp.path().join("verify-endpoint.sqlite")),
        },
    )
    .await
    .unwrap();

    assert!(vr.chunks_checked > 0);
    assert!(vr.bytes_checked > 0);
}

#[tokio::test]
async fn verify_fails_when_any_chunk_missing() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    write_file(
        source.join("a.txt"),
        b"hello world\nhello world\nhello world\n",
    );

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let master_key = [7u8; 32];

    let r1 = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path.clone(),
            filemap_dir: filemap_dir.clone(),
            source_path: source.clone(),
            label: "t1".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 256,
                avg_bytes: 1024,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key,
            snapshot_id: None,
            keep_last_snapshots: 10,
        },
    )
    .await
    .unwrap();

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();

    let manifest_object_id: String =
        sqlx::query("SELECT manifest_object_id FROM remote_indexes WHERE snapshot_id = ? LIMIT 1")
            .bind(&r1.snapshot_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("manifest_object_id");

    let endpoint_manifest_object_id: String =
        sqlx::query("SELECT value FROM endpoint_state WHERE key = ? LIMIT 1")
            .bind(televy_backup_core::index_sync::ENDPOINT_STATE_ENDPOINT_MANIFEST_OBJECT_ID_KEY)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("value");

    let row = sqlx::query(
        "SELECT chunk_hash, object_id FROM chunk_objects WHERE provider = 'test.mem' LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let chunk_hash: String = row.get("chunk_hash");
    let object_id: String = row.get("object_id");

    let underlying_object_id = match parse_chunk_object_ref(&object_id).unwrap() {
        ChunkObjectRef::Direct { object_id } => object_id,
        ChunkObjectRef::PackSlice { pack_object_id, .. } => pack_object_id,
    };
    storage.remove(&underlying_object_id).await;

    let err = verify_snapshot(
        &storage,
        VerifyConfig {
            snapshot_id: r1.snapshot_id.clone(),
            filemap_manifest_object_id: manifest_object_id,
            endpoint_manifest_object_id: Some(endpoint_manifest_object_id),
            endpoint_index_id: None,
            master_key,
            filemap_db_path: temp.path().join("verify-index.sqlite"),
            endpoint_db_path: Some(temp.path().join("verify-endpoint.sqlite")),
        },
    )
    .await
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains(&chunk_hash));
}
