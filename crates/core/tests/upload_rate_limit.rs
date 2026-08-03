use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use televy_backup_core::config::TelegramRateLimit;
use televy_backup_core::{
    BackupConfig, ChunkingConfig, Error, RemoteDedupeMode, Storage, run_backup,
};
use tempfile::TempDir;
use tokio::sync::Mutex;

fn write_file(path: PathBuf, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

struct TimedStorage {
    delay: Duration,
    concurrent: AtomicUsize,
    max_concurrent: AtomicUsize,
    starts: Mutex<Vec<Instant>>,
    attempts: Mutex<Vec<(Instant, Instant, usize)>>,
    counter: AtomicUsize,
    fail_first_large: AtomicUsize,
}

impl TimedStorage {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            concurrent: AtomicUsize::new(0),
            max_concurrent: AtomicUsize::new(0),
            starts: Mutex::new(Vec::new()),
            attempts: Mutex::new(Vec::new()),
            counter: AtomicUsize::new(0),
            fail_first_large: AtomicUsize::new(0),
        }
    }

    fn failing_first_large(delay: Duration) -> Self {
        let storage = Self::new(delay);
        storage.fail_first_large.store(1, Ordering::Relaxed);
        storage
    }

    fn max_concurrent(&self) -> usize {
        self.max_concurrent.load(Ordering::Relaxed)
    }

    async fn start_times(&self) -> Vec<Instant> {
        self.starts.lock().await.clone()
    }

    async fn attempts(&self) -> Vec<(Instant, Instant, usize)> {
        self.attempts.lock().await.clone()
    }
}

impl Storage for TimedStorage {
    fn provider(&self) -> &str {
        "test.timed"
    }

    fn upload_document<'a>(
        &'a self,
        _filename: &'a str,
        bytes: Vec<u8>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = televy_backup_core::Result<String>> + Send + 'a>,
    > {
        Box::pin(async move {
            let current = self.concurrent.fetch_add(1, Ordering::Relaxed) + 1;
            loop {
                let prev = self.max_concurrent.load(Ordering::Relaxed);
                if current <= prev {
                    break;
                }
                if self
                    .max_concurrent
                    .compare_exchange(prev, current, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    break;
                }
            }
            let started = Instant::now();
            self.starts.lock().await.push(started);
            let fails_this_attempt = bytes.len() > 1_000_000
                && self
                    .fail_first_large
                    .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                        remaining.checked_sub(1)
                    })
                    .is_ok();
            let delay = if fails_this_attempt {
                Duration::from_millis(20)
            } else {
                self.delay
            };
            if !delay.is_zero() {
                tokio::time::sleep(delay).await;
            }
            let finished = Instant::now();
            self.attempts
                .lock()
                .await
                .push((started, finished, bytes.len()));
            self.concurrent.fetch_sub(1, Ordering::Relaxed);
            if fails_this_attempt {
                return Err(Error::InvalidConfig {
                    message: "planned large upload failure".to_string(),
                });
            }
            let id = self.counter.fetch_add(1, Ordering::Relaxed);
            Ok(format!("timed:{id}"))
        })
    }

    fn download_document<'a>(
        &'a self,
        _object_id: &'a str,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = televy_backup_core::Result<Vec<u8>>> + Send + 'a>,
    > {
        Box::pin(async {
            Err(Error::InvalidConfig {
                message: "download not supported in TimedStorage".to_string(),
            })
        })
    }
}

fn fill_deterministic_noise(buf: &mut [u8], mut seed: u64) -> u64 {
    for byte in buf {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *byte = (seed >> 32) as u8;
    }
    seed
}

#[tokio::test]
async fn upload_concurrency_respects_limit() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    for i in 0..10u8 {
        write_file(source.join(format!("f{i}.bin")), &[i; 4096]);
    }

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let dedupe_db_path = temp.path().join("dedupe.sqlite");
    let dedupe_pending_db_path = temp.path().join("dedupe.pending.sqlite");
    let storage = TimedStorage::new(Duration::from_millis(50));

    run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path,
            dedupe_pending_db_path,
            source_path: source,
            label: "t".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: TelegramRateLimit {
                max_concurrent_uploads: 2,
                min_delay_ms: 0,
            },
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    let max_concurrent = storage.max_concurrent();
    assert!(max_concurrent <= 2);
    assert!(max_concurrent >= 2);
}

#[tokio::test]
async fn upload_min_delay_is_global() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    for i in 0..3u8 {
        write_file(source.join(format!("f{i}.bin")), &[i; 4096]);
    }

    let db_path = temp.path().join("index.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    let dedupe_db_path = temp.path().join("dedupe.sqlite");
    let dedupe_pending_db_path = temp.path().join("dedupe.pending.sqlite");
    let storage = TimedStorage::new(Duration::from_millis(0));

    run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: filemap_dir.clone(),
            dedupe_db_path,
            dedupe_pending_db_path,
            source_path: source,
            label: "t".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: TelegramRateLimit {
                max_concurrent_uploads: 1,
                min_delay_ms: 50,
            },
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    let starts = storage.start_times().await;
    for window in starts.windows(2) {
        let delta = window[1].duration_since(window[0]);
        assert!(delta >= Duration::from_millis(40));
    }
}

#[tokio::test]
async fn index_parts_overlap_within_the_global_upload_limit() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    write_file(source.join("single.bin"), &[42u8; 4096]);

    let db_path = temp.path().join("index.sqlite");
    let prep_pool = televy_backup_core::index_db::open_index_db(&db_path)
        .await
        .unwrap();
    let mut tx = prep_pool.begin().await.unwrap();
    let mut noise = vec![0u8; 512];
    let mut seed = 0xC0FFEE1234u64;
    for _ in 0..10_000 {
        seed = fill_deterministic_noise(&mut noise, seed);
        let chunk_hash = blake3::hash(&noise).to_hex().to_string();
        let mut object_id = String::from("tgfile:");
        for byte in &noise {
            use std::fmt::Write as _;
            let _ = write!(&mut object_id, "{byte:02x}");
        }
        sqlx::query(
            "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES (?, 1, 'blake3', 'xchacha20poly1305', '2026-01-01T00:00:00Z')",
        )
        .bind(&chunk_hash)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at) VALUES (?, 'test.timed', ?, '2026-01-01T00:00:00Z')",
        )
        .bind(&chunk_hash)
        .bind(object_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    prep_pool.close().await;

    let storage = TimedStorage::new(Duration::from_millis(80));
    let result = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: temp.path().join("filemaps"),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "index-concurrency".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: TelegramRateLimit {
                max_concurrent_uploads: 2,
                min_delay_ms: 0,
            },
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await
    .unwrap();

    assert!(
        result.index_parts >= 2,
        "fixture must produce multiple index parts"
    );
    assert_eq!(storage.max_concurrent(), 2);
    let attempts = storage.attempts().await;
    let index_parts: Vec<_> = attempts
        .iter()
        .filter(|(_, _, bytes)| *bytes > 1_000_000)
        .collect();
    assert!(
        index_parts.len() >= 2,
        "expected two large index-part RPCs, attempts={:?}",
        attempts
            .iter()
            .map(|(_, _, bytes)| bytes)
            .collect::<Vec<_>>()
    );
    assert!(index_parts.iter().enumerate().any(|(i, first)| {
        index_parts
            .iter()
            .skip(i + 1)
            .any(|second| first.0 < second.1 && second.0 < first.1)
    }));
    let last_part_finished = index_parts
        .iter()
        .map(|(_, finished, _)| *finished)
        .max()
        .unwrap();
    assert!(
        attempts
            .iter()
            .any(|(started, _, bytes)| { *bytes < 64 * 1024 && *started >= last_part_finished })
    );
}

#[tokio::test]
async fn index_part_failure_drains_started_uploads_before_returning() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    write_file(source.join("single.bin"), &[42u8; 4096]);

    let db_path = temp.path().join("index.sqlite");
    let prep_pool = televy_backup_core::index_db::open_index_db(&db_path)
        .await
        .unwrap();
    let mut tx = prep_pool.begin().await.unwrap();
    let mut noise = vec![0u8; 512];
    let mut seed = 0xC0FFEE1234u64;
    for _ in 0..10_000 {
        seed = fill_deterministic_noise(&mut noise, seed);
        let chunk_hash = blake3::hash(&noise).to_hex().to_string();
        let mut object_id = String::from("tgfile:");
        for byte in &noise {
            use std::fmt::Write as _;
            let _ = write!(&mut object_id, "{byte:02x}");
        }
        sqlx::query(
            "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES (?, 1, 'blake3', 'xchacha20poly1305', '2026-01-01T00:00:00Z')",
        )
        .bind(&chunk_hash)
        .execute(&mut *tx)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at) VALUES (?, 'test.timed', ?, '2026-01-01T00:00:00Z')",
        )
        .bind(&chunk_hash)
        .bind(object_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    prep_pool.close().await;

    let storage = TimedStorage::failing_first_large(Duration::from_millis(250));
    let result = run_backup(
        &storage,
        BackupConfig {
            endpoint_db_path: db_path,
            filemap_dir: temp.path().join("filemaps"),
            dedupe_db_path: temp.path().join("dedupe.sqlite"),
            dedupe_pending_db_path: temp.path().join("dedupe.pending.sqlite"),
            source_path: source,
            label: "index-failure-drain".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 4096,
                avg_bytes: 4096,
                max_bytes: 4096,
            },
            rate_limit: TelegramRateLimit {
                max_concurrent_uploads: 2,
                min_delay_ms: 0,
            },
            master_key: [8u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
            remote_dedupe: RemoteDedupeMode::Disabled,
        },
    )
    .await;

    assert!(result.is_err());
    let attempts = storage.attempts().await;
    assert!(
        attempts
            .iter()
            .filter(|(_, _, bytes)| *bytes > 1_000_000)
            .count()
            >= 2,
        "all started index parts must finish before the task returns"
    );
    assert_eq!(storage.concurrent.load(Ordering::Relaxed), 0);
}
