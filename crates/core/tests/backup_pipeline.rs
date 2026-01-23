use std::path::PathBuf;

use sqlx::Row;
use televy_backup_core::{BackupConfig, ChunkingConfig, InMemoryStorage, run_backup};
use tempfile::TempDir;

fn write_file(path: PathBuf, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn backup_pipeline_dedupes_chunks_across_runs() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    write_file(
        source.join("a.txt"),
        b"hello world\nhello world\nhello world\n",
    );
    write_file(source.join("nested/b.bin"), &[42u8; 10_000]);

    let db_path = temp.path().join("index.sqlite");

    let storage = InMemoryStorage::new();
    let chunking = ChunkingConfig {
        min_bytes: 64,
        avg_bytes: 256,
        max_bytes: 1024,
    };

    let cfg1 = BackupConfig {
        db_path: db_path.clone(),
        source_path: source.clone(),
        label: "t1".to_string(),
        chunking: chunking.clone(),
        rate_limit: Default::default(),
        master_key: [7u8; 32],
        snapshot_id: None,
        keep_last_snapshots: 10,
    };

    let r1 = run_backup(&storage, cfg1).await.unwrap();
    assert!(r1.chunks_uploaded > 0);
    assert!(r1.index_parts > 0);

    let uploads_after_r1 = storage.uploaded.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        uploads_after_r1 as u64,
        r1.data_objects_uploaded + r1.index_parts + 1
    );

    let cfg2 = BackupConfig {
        db_path: db_path.clone(),
        source_path: source.clone(),
        label: "t2".to_string(),
        chunking,
        rate_limit: Default::default(),
        master_key: [7u8; 32],
        snapshot_id: None,
        keep_last_snapshots: 10,
    };

    let r2 = run_backup(&storage, cfg2).await.unwrap();
    assert_eq!(r2.chunks_uploaded, 0);
    assert!(r2.bytes_deduped > 0);
    assert!(r2.index_parts > 0);

    let uploads_after_r2 = storage.uploaded.load(std::sync::atomic::Ordering::Relaxed);
    let delta = (uploads_after_r2 - uploads_after_r1) as u64;
    assert_eq!(delta, r2.data_objects_uploaded + r2.index_parts + 1);

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();

    let snapshots: i64 = sqlx::query("SELECT COUNT(*) as n FROM snapshots")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(snapshots, 2);

    let remote_indexes: i64 = sqlx::query("SELECT COUNT(*) as n FROM remote_indexes")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(remote_indexes, 2);

    let chunk_objects: i64 =
        sqlx::query("SELECT COUNT(*) as n FROM chunk_objects WHERE provider='test.mem'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("n");
    assert!(chunk_objects > 0);
}
