use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use sqlx::Row;
use televy_backup_core::{
    BackupConfig, BackupOptions, ChunkingConfig, InMemoryStorage, ProgressSink, RemoteDedupeMode,
    SourceQuickStats, TaskProgress, run_backup, run_backup_with,
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
    let filemap_dir = temp.path().join("filemaps");
    let dedupe_db_path = temp.path().join("dedupe.sqlite");
    let dedupe_pending_db_path = temp.path().join("dedupe.pending.sqlite");

    let storage = InMemoryStorage::new();
    let chunking = ChunkingConfig {
        min_bytes: 64,
        avg_bytes: 256,
        max_bytes: 1024,
    };

    let cfg1 = BackupConfig {
        endpoint_db_path: db_path.clone(),
        filemap_dir: filemap_dir.clone(),
        dedupe_db_path: dedupe_db_path.clone(),
        dedupe_pending_db_path: dedupe_pending_db_path.clone(),
        source_path: source.clone(),
        label: "t1".to_string(),
        chunking: chunking.clone(),
        rate_limit: Default::default(),
        master_key: [7u8; 32],
        snapshot_id: None,
        keep_last_snapshots: 10,
        remote_dedupe: RemoteDedupeMode::Disabled,
    };

    let r1 = run_backup(&storage, cfg1).await.unwrap();
    assert!(r1.chunks_uploaded > 0);
    assert!(r1.index_parts > 0);

    let uploads_after_r1 = storage.uploaded.load(std::sync::atomic::Ordering::Relaxed);
    assert_eq!(
        uploads_after_r1 as u64,
        // Two-level index uploads: filemap manifest + endpoint manifest.
        r1.data_objects_uploaded + r1.index_parts + 2
    );

    let cfg2 = BackupConfig {
        endpoint_db_path: db_path.clone(),
        filemap_dir: filemap_dir.clone(),
        dedupe_db_path: dedupe_db_path.clone(),
        dedupe_pending_db_path: dedupe_pending_db_path.clone(),
        source_path: source.clone(),
        label: "t2".to_string(),
        chunking,
        rate_limit: Default::default(),
        master_key: [7u8; 32],
        snapshot_id: None,
        keep_last_snapshots: 10,
        remote_dedupe: RemoteDedupeMode::Disabled,
    };

    let r2 = run_backup(&storage, cfg2).await.unwrap();
    assert_eq!(r2.chunks_uploaded, 0);
    assert_eq!(r2.bytes_read, 0);
    assert!(r2.bytes_deduped > 0);
    assert!(r2.index_parts > 0);

    let uploads_after_r2 = storage.uploaded.load(std::sync::atomic::Ordering::Relaxed);
    let delta = (uploads_after_r2 - uploads_after_r1) as u64;
    assert_eq!(delta, r2.data_objects_uploaded + r2.index_parts + 2);

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
    let filemap_dir = temp.path().join("filemaps");
    let dedupe_db_path = temp.path().join("dedupe.sqlite");
    let dedupe_pending_db_path = temp.path().join("dedupe.pending.sqlite");
    let storage = InMemoryStorage::new();
    let cfg = BackupConfig {
        endpoint_db_path: db_path.clone(),
        filemap_dir: filemap_dir.clone(),
        dedupe_db_path,
        dedupe_pending_db_path,
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
        remote_dedupe: RemoteDedupeMode::Disabled,
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

    let filemap_db_path = filemap_dir.join(format!("{}.sqlite", result.snapshot_id));
    let filemap_pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", filemap_db_path.display()))
        .await
        .unwrap();
    let files_in_snapshot: i64 =
        sqlx::query("SELECT COUNT(*) as n FROM files WHERE snapshot_id = ? AND kind = 'file'")
            .bind(&result.snapshot_id)
            .fetch_one(&filemap_pool)
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

#[tokio::test]
async fn backup_writes_normal_level_performance_intervals_to_run_log() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    for i in 0..11 {
        write_file(source.join(format!("f{i}.bin")), &[i as u8; 4096]);
    }

    let run_log = televy_backup_core::run_log::start_run_log(
        "backup",
        "performance-test",
        temp.path(),
        televy_backup_core::local_settings::NORMAL_FILTER,
    )
    .expect("start run log");
    let run_log_path = run_log.path().to_path_buf();
    let storage = InMemoryStorage::new();
    let result = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: temp.path().join("index.sqlite"),
            filemap_dir: temp.path().join("filemaps"),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "performance".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [4u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .expect("backup succeeds");
    assert!(result.data_objects_uploaded > 0);
    drop(run_log);

    let log_entries = std::fs::read_to_string(run_log_path)
        .expect("read run log")
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).expect("valid NDJSON"))
        .collect::<Vec<_>>();
    let events = log_entries
        .iter()
        .filter_map(|line| line["fields"]["event"].as_str().map(str::to_owned))
        .collect::<Vec<_>>();

    for expected in [
        "performance.scan.start",
        "performance.scan.trace",
        "performance.scan.finish",
        "performance.scan.queue_wait.start",
        "performance.scan.queue_wait.finish",
        "performance.upload.rate_limit_wait",
        "performance.upload.start",
        "performance.upload.finish",
        "performance.index.compression.start",
        "performance.index.compression.finish",
    ] {
        assert!(
            events.iter().any(|event| event == expected),
            "missing performance event {expected}: {events:?}"
        );
    }

    let scan_trace = log_entries
        .iter()
        .find(|line| line["fields"]["event"] == "performance.scan.trace")
        .expect("scan resource trace performance event");
    let trace: serde_json::Value = serde_json::from_str(
        scan_trace["fields"]["trace_json"]
            .as_str()
            .expect("scan trace JSON string"),
    )
    .expect("valid scan trace JSON");
    assert_eq!(trace["version"], 1);
    assert_eq!(trace["resolution_ms"], 1_000);
    assert_eq!(
        scan_trace["fields"]["resolution_ms"],
        trace["resolution_ms"]
    );
    let buckets = trace["buckets"].as_array().expect("scan trace buckets");
    assert!(
        !buckets.is_empty(),
        "scan trace must contain actual time slices"
    );
    assert!(
        buckets
            .iter()
            .any(|bucket| bucket["sqlite_us"].as_u64().unwrap_or(0) > 0),
        "scan trace must expose SQLite time slices"
    );
    assert!(
        buckets
            .iter()
            .any(|bucket| bucket["read_chunk_us"].as_u64().unwrap_or(0) > 0),
        "scan trace must expose read-chunk time slices"
    );
    assert!(
        buckets
            .iter()
            .all(|bucket| bucket.get("unattributed_us").is_none()),
        "unmeasured time must stay visibly absent from the resource trace"
    );

    let queue_wait_starts = log_entries
        .iter()
        .filter(|line| line["fields"]["event"] == "performance.scan.queue_wait.start")
        .map(|line| line["fields"]["queue_wait_id"].clone())
        .collect::<Vec<_>>();
    let queue_wait_finishes = log_entries
        .iter()
        .filter(|line| line["fields"]["event"] == "performance.scan.queue_wait.finish")
        .map(|line| line["fields"]["queue_wait_id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(queue_wait_starts, queue_wait_finishes);
}

#[tokio::test]
async fn backup_compacts_local_index_db_after_success() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    let file_path = source.join("single.bin");
    write_file(file_path.clone(), &[1u8; 4096]);

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let dedupe_db_path = temp.path().join("dedupe.sqlite");
    let dedupe_pending_db_path = temp.path().join("dedupe.pending.sqlite");
    let storage = InMemoryStorage::new();

    let cfg = BackupConfig {
        endpoint_db_path: db_path.clone(),
        filemap_dir: filemap_dir.clone(),
        dedupe_db_path,
        dedupe_pending_db_path,
        source_path: source.clone(),
        label: "compact".to_string(),
        chunking: ChunkingConfig {
            min_bytes: 4096,
            avg_bytes: 4096,
            max_bytes: 4096,
        },
        rate_limit: Default::default(),
        master_key: [3u8; 32],
        snapshot_id: None,
        keep_last_snapshots: 10,
        remote_dedupe: RemoteDedupeMode::Disabled,
    };

    let r1 = run_backup(&storage, cfg.clone()).await.unwrap();

    // Second run produces a newer snapshot for the same source_path.
    write_file(file_path, &[2u8; 4096]);
    let r2 = run_backup(&storage, cfg).await.unwrap();
    assert_ne!(r1.snapshot_id, r2.snapshot_id);

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();

    let snapshots: i64 = sqlx::query("SELECT COUNT(*) as n FROM snapshots")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(snapshots, 2);

    // Endpoint DB should not contain file maps (`files` / `file_chunks`) in two-level mode.
    let rows = sqlx::query("SELECT DISTINCT snapshot_id FROM files ORDER BY snapshot_id")
        .fetch_all(&pool)
        .await
        .unwrap();
    let kept = rows
        .into_iter()
        .map(|r| r.get::<String, _>("snapshot_id"))
        .collect::<Vec<_>>();
    assert_eq!(kept, Vec::<String>::new());

    let file_chunks: i64 = sqlx::query("SELECT COUNT(*) as n FROM file_chunks")
        .fetch_one(&pool)
        .await
        .unwrap()
        .get("n");
    assert_eq!(file_chunks, 0);
}
