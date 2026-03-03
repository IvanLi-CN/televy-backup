use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use sqlx::Row;
use televy_backup_core::{
    BackupConfig, BackupOptions, ChunkingConfig, Error, InMemoryStorage, ProgressSink,
    RemoteDedupeMode, Result, SourceQuickStats, Storage, TaskProgress, run_backup, run_backup_with,
};
use tempfile::TempDir;

fn write_file(path: PathBuf, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

async fn snapshot_count_for_source(pool: &sqlx::SqlitePool, source: &std::path::Path) -> i64 {
    sqlx::query("SELECT COUNT(*) as n FROM snapshots WHERE source_path = ?")
        .bind(source.to_string_lossy().as_ref())
        .fetch_one(pool)
        .await
        .unwrap()
        .get("n")
}

fn fill_deterministic_noise(buf: &mut [u8], mut seed: u64) -> u64 {
    for b in buf.iter_mut() {
        // xorshift64* gives stable pseudo-random bytes for test payloads.
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        let out = seed.wrapping_mul(0x2545F4914F6CDD1D);
        *b = (out & 0xFF) as u8;
    }
    seed
}

#[derive(Default)]
struct ProgressProbe {
    events: Mutex<Vec<TaskProgress>>,
}

impl ProgressSink for ProgressProbe {
    fn on_progress(&self, progress: TaskProgress) {
        self.events
            .lock()
            .expect("progress probe mutex poisoned")
            .push(progress);
    }
}

struct FailOnUpload<'a, S: Storage + Sync> {
    inner: &'a S,
    fail_on_call: usize,
    calls: AtomicUsize,
}

impl<'a, S: Storage + Sync> FailOnUpload<'a, S> {
    fn new(inner: &'a S, fail_on_call: usize) -> Self {
        Self {
            inner,
            fail_on_call,
            calls: AtomicUsize::new(0),
        }
    }
}

impl<'a, S: Storage + Sync> Storage for FailOnUpload<'a, S> {
    fn provider(&self) -> &str {
        self.inner.provider()
    }

    fn upload_document<'b>(
        &'b self,
        filename: &'b str,
        bytes: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'b>> {
        Box::pin(async move {
            let call_no = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
            if call_no == self.fail_on_call {
                return Err(Error::Telegram {
                    message: "injected upload failure".to_string(),
                });
            }
            self.inner.upload_document(filename, bytes).await
        })
    }

    fn download_document<'b>(
        &'b self,
        object_id: &'b str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>>> + Send + 'b>> {
        self.inner.download_document(object_id)
    }
}

struct FailOnRetryableUpload<'a, S: Storage + Sync> {
    inner: &'a S,
    fail_on_call: usize,
    fail_message: &'static str,
    calls: AtomicUsize,
}

impl<'a, S: Storage + Sync> FailOnRetryableUpload<'a, S> {
    fn new(inner: &'a S, fail_on_call: usize) -> Self {
        Self::with_message(inner, fail_on_call, "timed out injected upload failure")
    }

    fn with_message(inner: &'a S, fail_on_call: usize, fail_message: &'static str) -> Self {
        Self {
            inner,
            fail_on_call,
            fail_message,
            calls: AtomicUsize::new(0),
        }
    }
}

impl<'a, S: Storage + Sync> Storage for FailOnRetryableUpload<'a, S> {
    fn provider(&self) -> &str {
        self.inner.provider()
    }

    fn upload_document<'b>(
        &'b self,
        filename: &'b str,
        bytes: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'b>> {
        Box::pin(async move {
            let call_no = self.calls.fetch_add(1, Ordering::Relaxed) + 1;
            if call_no == self.fail_on_call {
                return Err(Error::Telegram {
                    message: self.fail_message.to_string(),
                });
            }
            self.inner.upload_document(filename, bytes).await
        })
    }

    fn download_document<'b>(
        &'b self,
        object_id: &'b str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>>> + Send + 'b>> {
        self.inner.download_document(object_id)
    }
}

#[tokio::test]
async fn pack_enabled_by_count_reduces_upload_calls() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    for i in 0..11u8 {
        write_file(source.join(format!("f{i}.bin")), &[i; 4096]);
    }

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let res = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "t".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    assert_eq!(res.chunks_uploaded, 11);
    assert_eq!(res.data_objects_uploaded, 1);

    let uploads = storage.uploaded.load(Ordering::Relaxed) as u64;
    // Two-level index uploads: filemap manifest + endpoint manifest.
    assert_eq!(uploads, res.data_objects_uploaded + res.index_parts + 2);
}

#[tokio::test]
async fn small_batch_does_not_enable_pack() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    for i in 0..10u8 {
        write_file(source.join(format!("f{i}.bin")), &[i; 4096]);
    }

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let res = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "t".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    assert_eq!(res.chunks_uploaded, 10);
    assert_eq!(res.data_objects_uploaded, 10);
}

#[tokio::test]
async fn packed_upload_source_bytes_match_source_need_upload_total() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    for i in 0..11u8 {
        write_file(source.join(format!("f{i}.bin")), &[i; 4096]);
    }

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let probe = ProgressProbe::default();

    let res = run_backup_with(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "pack-source-bytes".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
        BackupOptions {
            cancel: None,
            progress: Some(&probe),
            source_quick_stats: Some(SourceQuickStats {
                files_total: 11,
                bytes_total: 11 * 4096,
            }),
        },
    )
    .await
    .unwrap();

    assert_eq!(res.chunks_uploaded, 11);
    assert_eq!(res.data_objects_uploaded, 1);

    let events = probe.events.lock().expect("progress probe mutex poisoned");
    let max_source_need_upload = events
        .iter()
        .filter_map(|p| p.source_bytes_need_upload_total)
        .max()
        .expect("missing source_bytes_need_upload_total");
    let max_uploaded_source = events
        .iter()
        .filter_map(|p| p.bytes_uploaded_source)
        .max()
        .expect("missing bytes_uploaded_source");

    assert_eq!(max_source_need_upload, 11 * 4096);
    assert_eq!(
        max_uploaded_source, max_source_need_upload,
        "bytes_uploaded_source should count source payload bytes, not encrypted overhead"
    );
}

#[tokio::test]
async fn large_index_db_uploads_multiple_index_parts() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    write_file(source.join("single.bin"), &[42u8; 4096]);

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    // Populate exported schema tables (chunks/chunk_objects) with noise so the *compact* index
    // export is forced into multiple uploaded parts.
    let prep_pool = televy_backup_core::index_db::open_index_db(&db_path)
        .await
        .unwrap();

    let mut noise = vec![0u8; 512];
    let mut seed = 0xC0FFEE1234u64;
    let mut tx = prep_pool.begin().await.unwrap();
    for _ in 0..5_000 {
        seed = fill_deterministic_noise(&mut noise, seed);
        let chunk_hash = blake3::hash(&noise).to_hex().to_string();

        let mut object_id = String::with_capacity("tgfile:".len() + noise.len() * 2);
        object_id.push_str("tgfile:");
        for b in &noise {
            use std::fmt::Write as _;
            let _ = write!(&mut object_id, "{:02x}", b);
        }

        sqlx::query(
            "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES (?, 1, 'blake3', 'xchacha20poly1305', '2026-01-01T00:00:00Z')",
        )
        .bind(&chunk_hash)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at) VALUES (?, 'test.mem', ?, '2026-01-01T00:00:00Z')",
        )
        .bind(&chunk_hash)
        .bind(object_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    prep_pool.close().await;

    let storage = InMemoryStorage::new();
    let res = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "idx-large".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    assert!(
        res.index_parts >= 2,
        "expected multi-part index upload for large local index db, got {}",
        res.index_parts
    );
}

#[tokio::test]
async fn restart_after_index_upload_failure_does_not_reupload_chunks() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    for i in 0..11u8 {
        write_file(source.join(format!("f{i}.bin")), &[i; 4096]);
    }

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();

    // Fail on the 2nd upload: first pack upload succeeds, then index part upload fails.
    let failing = FailOnUpload::new(&storage, 2);
    let err = run_backup(
        &failing,
        BackupConfig {
            endpoint_db_path: db_path.clone(),
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source.clone(),
            label: "t1".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("injected upload failure"));

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();
    let chunk_objects: i64 =
        sqlx::query("SELECT COUNT(*) as n FROM chunk_objects WHERE provider='test.mem'")
            .fetch_one(&pool)
            .await
            .unwrap()
            .get("n");
    assert!(chunk_objects > 0);

    // Re-run without failure. Data chunks should be fully deduped.
    let res2 = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path.clone(),
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "t2".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    assert_eq!(res2.chunks_uploaded, 0);
    assert_eq!(res2.data_objects_uploaded, 0);
    assert!(res2.bytes_deduped > 0);
}

#[tokio::test]
async fn upload_retries_after_network_unreachable_failure() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let failing = FailOnRetryableUpload::with_message(
        &storage,
        1,
        "network is unreachable injected upload failure",
    );

    let res = run_backup(
        &failing,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "idx-retry-network-unreachable".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .expect("expected network unreachable upload to retry and succeed");

    assert!(res.index_parts >= 1);
}

#[tokio::test]
async fn index_part_upload_retries_after_transient_failure() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let failing = FailOnRetryableUpload::new(&storage, 1);

    let res = run_backup(
        &failing,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "idx-retry-part".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .expect("expected index part upload to retry and succeed");

    assert!(res.index_parts >= 1);
}

#[tokio::test]
async fn index_manifest_upload_retries_after_transient_failure() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let failing = FailOnRetryableUpload::new(&storage, 2);

    let res = run_backup(
        &failing,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "idx-retry-manifest".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .expect("expected index manifest upload to retry and succeed");

    assert!(res.index_parts >= 1);
}

#[tokio::test]
async fn retention_preflight_bounds_snapshot_growth_on_repeated_failures() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    write_file(source.join("volatile.bin"), &[7u8; 4096]);

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let cfg = BackupConfig {
        endpoint_db_path: db_path.clone(),
        filemap_dir: filemap_dir.clone(),
        dedupe_db_path: temp.path().join("dedupe.sqlite"),
        dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
        source_path: source.clone(),
        label: "fail".to_string(),
        chunking: ChunkingConfig {
            min_bytes: 4096,
            avg_bytes: 4096,
            max_bytes: 4096,
        },
        rate_limit: Default::default(),
        master_key: [9u8; 32],
        snapshot_id: None,
        keep_last_snapshots: 2,
        remote_dedupe: RemoteDedupeMode::Disabled,
    };

    for _ in 0..6 {
        let failing = FailOnUpload::new(&storage, 1);
        let err = run_backup(&failing, cfg.clone()).await.unwrap_err();
        assert!(err.to_string().contains("injected upload failure"));
    }

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();
    let snapshots = snapshot_count_for_source(&pool, &source).await;
    assert!(
        snapshots <= 3,
        "expected repeated failed runs to stay bounded at keep_last_snapshots + 1, got {snapshots}"
    );
}

#[tokio::test]
async fn retention_preflight_does_not_prune_other_sources_before_backup() {
    let temp = TempDir::new().unwrap();
    let source_sync = temp.path().join("sync");
    let source_projects = temp.path().join("projects");
    std::fs::create_dir_all(&source_sync).unwrap();
    std::fs::create_dir_all(&source_projects).unwrap();
    write_file(source_sync.join("a.bin"), &[1u8; 4096]);
    write_file(source_projects.join("p.bin"), &[2u8; 4096]);

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();

    let base_chunking = ChunkingConfig {
        min_bytes: 4096,
        avg_bytes: 4096,
        max_bytes: 4096,
    };

    for i in 0..4 {
        run_backup(
            &storage,
            BackupConfig {
                endpoint_db_path: db_path.clone(),
                filemap_dir: filemap_dir.clone(),
                dedupe_db_path: temp.path().join("dedupe.sqlite"),
                dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
                source_path: source_projects.clone(),
                label: format!("projects-{i}"),
                chunking: base_chunking.clone(),
                rate_limit: Default::default(),
                master_key: [3u8; 32],
                snapshot_id: None,
                keep_last_snapshots: 10,
                remote_dedupe: RemoteDedupeMode::Disabled,
            },
        )
        .await
        .unwrap();
    }

    for i in 0..3 {
        run_backup(
            &storage,
            BackupConfig {
                endpoint_db_path: db_path.clone(),
                filemap_dir: filemap_dir.clone(),
                dedupe_db_path: temp.path().join("dedupe.sqlite"),
                dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
                source_path: source_sync.clone(),
                label: format!("sync-{i}"),
                chunking: base_chunking.clone(),
                rate_limit: Default::default(),
                master_key: [3u8; 32],
                snapshot_id: None,
                keep_last_snapshots: 10,
                remote_dedupe: RemoteDedupeMode::Disabled,
            },
        )
        .await
        .unwrap();
    }

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();
    assert_eq!(snapshot_count_for_source(&pool, &source_projects).await, 4);
    assert_eq!(snapshot_count_for_source(&pool, &source_sync).await, 3);

    run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path.clone(),
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source_sync.clone(),
            label: "sync-trim".to_string(),
            chunking: base_chunking,
            rate_limit: Default::default(),
            master_key: [3u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 2,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    let projects_after = snapshot_count_for_source(&pool, &source_projects).await;
    let sync_after = snapshot_count_for_source(&pool, &source_sync).await;
    assert_eq!(
        projects_after, 4,
        "expected retention to only prune the active source (idle sources are cleaned when they run)"
    );
    assert_eq!(
        sync_after, 2,
        "expected target source to respect keep_last_snapshots"
    );
}

#[tokio::test]
async fn retention_preflight_handles_large_backlog_with_batched_prune() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("sync");
    std::fs::create_dir_all(&source).unwrap();
    write_file(source.join("payload.bin"), &[5u8; 4096]);

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let storage = InMemoryStorage::new();
    let chunking = ChunkingConfig {
        min_bytes: 4096,
        avg_bytes: 4096,
        max_bytes: 4096,
    };

    for i in 0..18 {
        run_backup(
            &storage,
            BackupConfig {
                endpoint_db_path: db_path.clone(),
                filemap_dir: filemap_dir.clone(),
                dedupe_db_path: temp.path().join("dedupe.sqlite"),
                dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
                source_path: source.clone(),
                label: format!("seed-{i}"),
                chunking: chunking.clone(),
                rate_limit: Default::default(),
                master_key: [5u8; 32],
                snapshot_id: None,
                keep_last_snapshots: 64,
                remote_dedupe: RemoteDedupeMode::Disabled,
            },
        )
        .await
        .unwrap();
    }

    let pool = sqlx::SqlitePool::connect(&format!("sqlite:{}", db_path.display()))
        .await
        .unwrap();
    assert_eq!(snapshot_count_for_source(&pool, &source).await, 18);

    run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path.clone(),
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source.clone(),
            label: "trim".to_string(),
            chunking,
            rate_limit: Default::default(),
            master_key: [5u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 2,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        snapshot_count_for_source(&pool, &source).await,
        2,
        "expected batched retention pruning to keep only the configured snapshots"
    );
}
