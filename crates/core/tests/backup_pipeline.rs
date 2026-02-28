use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::Row;
use televy_backup_core::{
    BackupConfig, BackupOptions, ChunkingConfig, InMemoryStorage, ProgressSink, SourceQuickStats,
    TaskProgress, run_backup, run_backup_with,
};
use tempfile::TempDir;

fn write_file(path: PathBuf, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

struct MutateOnUpload {
    file_path: PathBuf,
    bytes: Vec<u8>,
    fired: AtomicBool,
    seen: Mutex<Vec<TaskProgress>>,
}

impl MutateOnUpload {
    fn new(file_path: impl AsRef<Path>, bytes: Vec<u8>) -> Self {
        Self {
            file_path: file_path.as_ref().to_path_buf(),
            bytes,
            fired: AtomicBool::new(false),
            seen: Mutex::new(Vec::new()),
        }
    }
}

impl ProgressSink for MutateOnUpload {
    fn on_progress(&self, progress: TaskProgress) {
        self.seen
            .lock()
            .expect("progress sink mutex poisoned")
            .push(progress.clone());
        if (progress.phase == "upload" || progress.phase == "scan_upload")
            && !self.fired.swap(true, Ordering::SeqCst)
        {
            std::fs::write(&self.file_path, &self.bytes).unwrap();
        }
    }
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
    assert_eq!(r2.bytes_read, 0);
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

#[tokio::test]
async fn backup_uploads_while_scanning_when_source_changes_mid_run() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    let file_path = source.join("volatile.bin");
    let mut initial = vec![0u8; 8 * 1024 * 1024];
    for (idx, byte) in initial.iter_mut().enumerate() {
        *byte = (idx as u8)
            .wrapping_mul(31)
            .wrapping_add(((idx >> 7) & 0xFF) as u8);
    }
    let changed = vec![0x33u8; initial.len()];
    write_file(file_path.clone(), &initial);

    let db_path = temp.path().join("index.sqlite");
    let storage = InMemoryStorage::new();
    let cfg = BackupConfig {
        db_path: db_path.clone(),
        source_path: source.clone(),
        label: "volatile".to_string(),
        chunking: ChunkingConfig {
            min_bytes: 64,
            avg_bytes: 256,
            max_bytes: 1024,
        },
        rate_limit: Default::default(),
        master_key: [9u8; 32],
        snapshot_id: None,
        keep_last_snapshots: 10,
    };

    let sink = MutateOnUpload::new(&file_path, changed);
    let result = run_backup_with(
        &storage,
        cfg,
        BackupOptions {
            cancel: None,
            progress: Some(&sink),
            source_quick_stats: Some(SourceQuickStats {
                files_total: 1,
                bytes_total: initial.len() as u64,
            }),
        },
    )
    .await
    .unwrap();

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();

    let files_in_snapshot: i64 =
        sqlx::query("SELECT COUNT(*) as n FROM files WHERE snapshot_id = ? AND kind = 'file'")
            .bind(&result.snapshot_id)
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("n");

    assert_eq!(files_in_snapshot, 1);
    assert!(result.chunks_uploaded > 0);

    let seen = sink.seen.lock().expect("progress sink mutex poisoned");
    let overlapped = seen.iter().any(|p| {
        p.bytes_uploaded.unwrap_or(0) > 0
            && p.bytes_read.unwrap_or(u64::MAX) < initial.len() as u64
            && (p.phase == "scan_upload" || p.phase == "upload")
    });
    assert!(
        overlapped,
        "expected upload bytes to advance before scan bytes reached source total"
    );
}
