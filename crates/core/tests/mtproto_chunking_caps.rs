use std::path::PathBuf;

use televy_backup_core::{BackupConfig, ChunkingConfig, InMemoryStorage, Storage, run_backup};
use tempfile::TempDir;

const MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES: usize = 128 * 1024 * 1024;
const FRAMING_OVERHEAD_BYTES: usize = 1 + 24 + 16;

struct ProviderOverride<'a, S: Storage + Sync> {
    inner: &'a S,
    provider: &'a str,
}

impl<'a, S: Storage + Sync> Storage for ProviderOverride<'a, S> {
    fn provider(&self) -> &str {
        self.provider
    }

    fn upload_document<'b>(
        &'b self,
        filename: &'b str,
        bytes: Vec<u8>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = televy_backup_core::Result<String>> + Send + 'b>>
    {
        self.inner.upload_document(filename, bytes)
    }

    fn download_document<'b>(
        &'b self,
        object_id: &'b str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = televy_backup_core::Result<Vec<u8>>> + Send + 'b>>
    {
        self.inner.download_document(object_id)
    }
}

fn write_file(path: PathBuf, bytes: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[tokio::test]
async fn mtproto_provider_chunking_max_bytes_over_limit_fails_fast() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    write_file(source.join("a.bin"), b"a");

    let db_path = temp.path().join("index.sqlite");
    let storage = InMemoryStorage::new();
    let mtproto = ProviderOverride {
        inner: &storage,
        provider: "telegram.mtproto/e1",
    };

    let max_plain = MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES - FRAMING_OVERHEAD_BYTES;

    let err = run_backup(
        &mtproto,
        BackupConfig {
            db_path,
            source_path: source,
            label: "t".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 1024 * 1024,
                avg_bytes: 4 * 1024 * 1024,
                max_bytes: (max_plain as u32) + 1,
            },
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
        },
    )
    .await
    .unwrap_err();

    let msg = err.to_string();
    assert!(msg.contains("MTProtoEngineeredUploadMaxBytes"));
    assert!(msg.contains("framing_overhead"));
    assert!(msg.contains("41"));
}

#[tokio::test]
async fn mtproto_provider_chunking_max_bytes_allows_exact_limit() {
    let temp = TempDir::new().unwrap();
    let source = temp.path().join("src");
    std::fs::create_dir_all(&source).unwrap();
    write_file(source.join("a.bin"), b"a");

    let db_path = temp.path().join("index.sqlite");
    let storage = InMemoryStorage::new();
    let mtproto = ProviderOverride {
        inner: &storage,
        provider: "telegram.mtproto/e1",
    };

    let max_plain = MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES - FRAMING_OVERHEAD_BYTES;

    run_backup(
        &mtproto,
        BackupConfig {
            db_path,
            source_path: source,
            label: "t".to_string(),
            chunking: ChunkingConfig {
                min_bytes: 1024 * 1024,
                avg_bytes: 4 * 1024 * 1024,
                max_bytes: max_plain as u32,
            },
            master_key: [7u8; 32],
            snapshot_id: None,
            keep_last_snapshots: 10,
        },
    )
    .await
    .unwrap();
}
