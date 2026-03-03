use std::path::{Path, PathBuf};

use sqlx::Row;
use televy_backup_core::{
    BackupConfig, ChunkingConfig, InMemoryStorage, RemoteDedupeMode, compute_source_quick_stats,
    run_backup,
};
use tempfile::TempDir;

fn write_file(path: PathBuf, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

fn base_backup_config(temp: &TempDir, source: &Path) -> BackupConfig {
    BackupConfig {
        endpoint_db_path: temp.path().join("index.sqlite"),
        filemap_dir: temp.path().join("filemaps"),
        dedupe_db_path: temp.path().join("dedupe.sqlite"),
        dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
        source_path: source.to_path_buf(),
        label: "televyignore".to_string(),
        chunking: ChunkingConfig {
            min_bytes: 64,
            avg_bytes: 256,
            max_bytes: 1024,
        },
        rate_limit: Default::default(),
        master_key: [7u8; 32],
        snapshot_id: None,
        keep_last_snapshots: 10,
        remote_dedupe: RemoteDedupeMode::Disabled,
    }
}

async fn snapshot_files(filemap_dir: &Path, snapshot_id: &str) -> Vec<(String, i64)> {
    let db_path = filemap_dir.join(format!("{snapshot_id}.sqlite"));
    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();
    let rows = sqlx::query(
        "SELECT path, size FROM files WHERE snapshot_id = ? AND kind = 'file' ORDER BY path",
    )
    .bind(snapshot_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    rows.into_iter()
        .map(|r| (r.get("path"), r.get("size")))
        .collect::<Vec<_>>()
}

#[tokio::test]
async fn backup_respects_televyignore_file_and_dir_patterns() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    write_file(source.join(".televyignore"), b"ignored.tmp\ncache/\n");
    write_file(source.join("kept.txt"), b"keep");
    write_file(source.join("ignored.tmp"), b"drop");
    write_file(source.join("cache").join("inner.txt"), b"drop-dir");

    let storage = InMemoryStorage::new();
    let cfg = base_backup_config(&temp, &source);
    let filemap_dir = cfg.filemap_dir.clone();
    let result = run_backup(&storage, cfg).await.unwrap();
    let files = snapshot_files(&filemap_dir, &result.snapshot_id).await;
    let file_paths = files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>();

    assert!(file_paths.contains(&"kept.txt"));
    assert!(!file_paths.contains(&"ignored.tmp"));
    assert!(!file_paths.contains(&"cache/inner.txt"));
}

#[tokio::test]
async fn backup_respects_televyignore_negation() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    write_file(source.join(".televyignore"), b"*.log\n!important.log\n");
    write_file(source.join("debug.log"), b"drop");
    write_file(source.join("important.log"), b"keep");

    let storage = InMemoryStorage::new();
    let cfg = base_backup_config(&temp, &source);
    let filemap_dir = cfg.filemap_dir.clone();
    let result = run_backup(&storage, cfg).await.unwrap();
    let files = snapshot_files(&filemap_dir, &result.snapshot_id).await;
    let file_paths = files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>();

    assert!(!file_paths.contains(&"debug.log"));
    assert!(file_paths.contains(&"important.log"));
}

#[tokio::test]
async fn backup_respects_nested_televyignore_precedence() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(source.join("nested")).unwrap();

    write_file(source.join(".televyignore"), b"nested/*.txt\n");
    write_file(source.join("nested/.televyignore"), b"!keep.txt\n");
    write_file(source.join("nested/keep.txt"), b"keep");
    write_file(source.join("nested/drop.txt"), b"drop");

    let storage = InMemoryStorage::new();
    let cfg = base_backup_config(&temp, &source);
    let filemap_dir = cfg.filemap_dir.clone();
    let result = run_backup(&storage, cfg).await.unwrap();
    let files = snapshot_files(&filemap_dir, &result.snapshot_id).await;
    let file_paths = files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>();

    assert!(file_paths.contains(&"nested/keep.txt"));
    assert!(!file_paths.contains(&"nested/drop.txt"));
}

#[tokio::test]
async fn quick_stats_respects_televyignore() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    write_file(
        source.join(".televyignore"),
        b".televyignore\nignored.bin\n",
    );
    write_file(source.join("included.bin"), b"12345");
    write_file(source.join("ignored.bin"), b"this-should-not-count");

    let quick = compute_source_quick_stats(&source, None).unwrap();

    let storage = InMemoryStorage::new();
    let cfg = base_backup_config(&temp, &source);
    let filemap_dir = cfg.filemap_dir.clone();
    let result = run_backup(&storage, cfg).await.unwrap();
    let files = snapshot_files(&filemap_dir, &result.snapshot_id).await;
    let scan_file_count = files.len() as u64;
    let scan_total_bytes = files.iter().map(|(_, size)| *size as u64).sum::<u64>();

    assert_eq!(quick.files_total, scan_file_count);
    assert_eq!(quick.bytes_total, scan_total_bytes);
}

#[tokio::test]
async fn invalid_televyignore_line_warns_and_continues() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    write_file(source.join(".televyignore"), b"[abc\n*.tmp\n");
    write_file(source.join("keep.txt"), b"keep");
    write_file(source.join("drop.tmp"), b"drop");

    let storage = InMemoryStorage::new();
    let cfg = base_backup_config(&temp, &source);
    let filemap_dir = cfg.filemap_dir.clone();
    let result = run_backup(&storage, cfg).await.unwrap();
    let files = snapshot_files(&filemap_dir, &result.snapshot_id).await;
    let file_paths = files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>();

    assert!(file_paths.contains(&"keep.txt"));
    assert!(!file_paths.contains(&"drop.tmp"));
}

#[tokio::test]
async fn hidden_files_not_implicitly_excluded() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    write_file(source.join(".hidden.txt"), b"hidden");
    write_file(source.join("visible.txt"), b"visible");

    let storage = InMemoryStorage::new();
    let cfg = base_backup_config(&temp, &source);
    let filemap_dir = cfg.filemap_dir.clone();
    let result = run_backup(&storage, cfg).await.unwrap();
    let files = snapshot_files(&filemap_dir, &result.snapshot_id).await;
    let file_paths = files.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>();

    assert!(file_paths.contains(&".hidden.txt"));
    assert!(file_paths.contains(&"visible.txt"));
}
