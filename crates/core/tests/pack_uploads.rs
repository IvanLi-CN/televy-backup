use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use sqlx::Row;
use televy_backup_core::{
    BackupConfig, ChunkingConfig, Error, InMemoryStorage, Result, Storage, run_backup,
};
use tempfile::TempDir;

fn write_file(path: PathBuf, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
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

#[tokio::test]
async fn pack_enabled_by_count_reduces_upload_calls() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();

    for i in 0..11u8 {
        write_file(source.join(format!("f{i}.bin")), &[i; 4096]);
    }

    let db_path = temp.path().join("index.sqlite");
    let storage = InMemoryStorage::new();
    let res = run_backup(
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
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
        },
    )
    .await
    .unwrap();

    assert_eq!(res.chunks_uploaded, 11);
    assert_eq!(res.data_objects_uploaded, 1);

    let uploads = storage.uploaded.load(Ordering::Relaxed) as u64;
    assert_eq!(uploads, res.data_objects_uploaded + res.index_parts + 1);
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
    let storage = InMemoryStorage::new();
    let res = run_backup(
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
            rate_limit: Default::default(),
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
        },
    )
    .await
    .unwrap();

    assert_eq!(res.chunks_uploaded, 10);
    assert_eq!(res.data_objects_uploaded, 10);
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
    let storage = InMemoryStorage::new();

    // Fail on the 2nd upload: first pack upload succeeds, then index part upload fails.
    let failing = FailOnUpload::new(&storage, 2);
    let err = run_backup(
        &failing,
        BackupConfig {
            db_path: db_path.clone(),
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
            db_path: db_path.clone(),
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
        },
    )
    .await
    .unwrap();

    assert_eq!(res2.chunks_uploaded, 0);
    assert_eq!(res2.data_objects_uploaded, 0);
    assert!(res2.bytes_deduped > 0);
}
