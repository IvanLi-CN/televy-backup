use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use televy_backup_core::config::TelegramRateLimit;
use televy_backup_core::{BackupConfig, ChunkingConfig, Error, Storage, run_backup};
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
    counter: AtomicUsize,
}

impl TimedStorage {
    fn new(delay: Duration) -> Self {
        Self {
            delay,
            concurrent: AtomicUsize::new(0),
            max_concurrent: AtomicUsize::new(0),
            starts: Mutex::new(Vec::new()),
            counter: AtomicUsize::new(0),
        }
    }

    fn max_concurrent(&self) -> usize {
        self.max_concurrent.load(Ordering::Relaxed)
    }

    async fn start_times(&self) -> Vec<Instant> {
        self.starts.lock().await.clone()
    }
}

impl Storage for TimedStorage {
    fn provider(&self) -> &str {
        "test.timed"
    }

    fn upload_document<'a>(
        &'a self,
        _filename: &'a str,
        _bytes: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = televy_backup_core::Result<String>> + Send + 'a>>
    {
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
            self.starts.lock().await.push(Instant::now());
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            self.concurrent.fetch_sub(1, Ordering::Relaxed);
            let id = self.counter.fetch_add(1, Ordering::Relaxed);
            Ok(format!("timed:{id}"))
        })
    }

    fn download_document<'a>(
        &'a self,
        _object_id: &'a str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = televy_backup_core::Result<Vec<u8>>> + Send + 'a>>
    {
        Box::pin(async {
            Err(Error::InvalidConfig {
                message: "download not supported in TimedStorage".to_string(),
            })
        })
    }
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
    let storage = TimedStorage::new(Duration::from_millis(50));

    run_backup(
        &storage,
        BackupConfig {
            db_path,
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
    let storage = TimedStorage::new(Duration::from_millis(0));

    run_backup(
        &storage,
        BackupConfig {
            db_path,
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
