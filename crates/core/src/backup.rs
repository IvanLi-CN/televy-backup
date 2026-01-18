use std::fs::File;
use std::path::{Path, PathBuf};

use fastcdc::v2020::StreamCDC;
use sqlx::{Row, SqlitePool, sqlite::SqliteRow};
use walkdir::WalkDir;

use crate::crypto::encrypt_framed;
use crate::index_db::open_index_db;
use crate::index_manifest::{IndexManifest, IndexManifestPart, index_part_aad};
use crate::progress::{ProgressSink, TaskProgress};
use crate::storage::Storage;
use crate::{Error, Result};
use tokio_util::sync::CancellationToken;

const TELEGRAM_BOTAPI_MAX_FILE_BYTES: usize = 50 * 1024 * 1024;
const INDEX_PART_BYTES: usize = 32 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ChunkingConfig {
    pub min_bytes: u32,
    pub avg_bytes: u32,
    pub max_bytes: u32,
}

impl ChunkingConfig {
    pub fn validate(&self) -> Result<()> {
        if self.min_bytes == 0 || self.avg_bytes == 0 || self.max_bytes == 0 {
            return Err(Error::InvalidConfig {
                message: "chunk sizes must be > 0".to_string(),
            });
        }
        if !(self.min_bytes <= self.avg_bytes && self.avg_bytes <= self.max_bytes) {
            return Err(Error::InvalidConfig {
                message: "chunk sizes must satisfy min <= avg <= max".to_string(),
            });
        }

        // Framing overhead: 1(version) + 24(nonce) + 16(tag)
        let max_framed = (self.max_bytes as usize) + 1 + 24 + 16;
        if max_framed > TELEGRAM_BOTAPI_MAX_FILE_BYTES {
            return Err(Error::InvalidConfig {
                message: format!(
                    "max chunk too large for Telegram Bot API (max_framed={max_framed} > {TELEGRAM_BOTAPI_MAX_FILE_BYTES})"
                ),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct BackupConfig {
    pub db_path: PathBuf,
    pub source_path: PathBuf,
    pub label: String,
    pub chunking: ChunkingConfig,
    pub master_key: [u8; 32],
    pub snapshot_id: Option<String>,
}

#[derive(Debug, Default, Clone)]
pub struct BackupResult {
    pub snapshot_id: String,
    pub files_total: u64,
    pub files_indexed: u64,
    pub chunks_total: u64,
    pub chunks_uploaded: u64,
    pub bytes_read: u64,
    pub bytes_uploaded: u64,
    pub bytes_deduped: u64,
    pub index_parts: u64,
}

pub async fn run_backup<S: Storage>(storage: &S, config: BackupConfig) -> Result<BackupResult> {
    run_backup_with(storage, config, BackupOptions::default()).await
}

#[derive(Default)]
pub struct BackupOptions<'a> {
    pub cancel: Option<&'a CancellationToken>,
    pub progress: Option<&'a dyn ProgressSink>,
}

pub async fn run_backup_with<S: Storage>(
    storage: &S,
    config: BackupConfig,
    options: BackupOptions<'_>,
) -> Result<BackupResult> {
    config.chunking.validate()?;
    if !config.source_path.is_dir() {
        return Err(Error::InvalidConfig {
            message: "source_path must be an existing directory".to_string(),
        });
    }

    let pool = open_index_db(&config.db_path).await?;
    let provider = storage.provider();

    let base_snapshot_id = latest_snapshot_for_source(&pool, &config.source_path).await?;
    let snapshot_id = config
        .snapshot_id
        .clone()
        .unwrap_or_else(|| format!("snp_{}", uuid::Uuid::new_v4()));

    sqlx::query(
        r#"
        INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id)
        VALUES (?, strftime('%Y-%m-%dT%H:%M:%fZ','now'), ?, ?, ?)
        "#,
    )
    .bind(&snapshot_id)
    .bind(path_to_utf8(&config.source_path)?)
    .bind(&config.label)
    .bind(base_snapshot_id)
    .execute(&pool)
    .await?;

    let mut result = BackupResult {
        snapshot_id: snapshot_id.clone(),
        ..BackupResult::default()
    };

    if let Some(sink) = options.progress {
        sink.on_progress(TaskProgress {
            phase: "scan".to_string(),
            files_total: None,
            files_done: Some(0),
            chunks_total: Some(0),
            chunks_done: Some(0),
            bytes_read: Some(0),
            bytes_uploaded: Some(0),
            bytes_deduped: Some(0),
        });
    }

    for entry in WalkDir::new(&config.source_path).follow_links(false) {
        if let Some(cancel) = options.cancel
            && cancel.is_cancelled()
        {
            return Err(Error::Cancelled);
        }

        let entry = entry.map_err(|e| Error::InvalidConfig {
            message: format!("walkdir error: {e}"),
        })?;

        let path = entry.path();
        if path == config.source_path {
            continue;
        }

        let rel_path =
            path.strip_prefix(&config.source_path)
                .map_err(|_| Error::InvalidConfig {
                    message: "path strip_prefix failed".to_string(),
                })?;
        let rel_path_str = path_to_utf8(rel_path)?;

        let metadata = entry.metadata()?;

        let kind = if metadata.is_dir() {
            "dir"
        } else if metadata.is_file() {
            "file"
        } else if metadata.is_symlink() {
            "symlink"
        } else {
            continue;
        };

        let (size, mtime_ms, mode) = if kind == "file" {
            let size = metadata.len() as i64;
            let mtime_ms = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::MetadataExt;
                metadata.mode() as i64
            };
            #[cfg(not(unix))]
            let mode = 0i64;
            (size, mtime_ms, mode)
        } else {
            (0i64, 0i64, 0i64)
        };

        result.files_total += 1;

        let file_id = format!("f_{}", uuid::Uuid::new_v4());
        sqlx::query(
            r#"
            INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind)
            VALUES (?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&file_id)
        .bind(&snapshot_id)
        .bind(rel_path_str)
        .bind(size)
        .bind(mtime_ms)
        .bind(mode)
        .bind(kind)
        .execute(&pool)
        .await?;

        result.files_indexed += 1;

        if let Some(sink) = options.progress {
            sink.on_progress(TaskProgress {
                phase: "scan".to_string(),
                files_total: None,
                files_done: Some(result.files_indexed),
                chunks_total: Some(result.chunks_total),
                chunks_done: Some(result.chunks_total),
                bytes_read: Some(result.bytes_read),
                bytes_uploaded: Some(result.bytes_uploaded),
                bytes_deduped: Some(result.bytes_deduped),
            });
        }

        if kind != "file" {
            continue;
        }

        let file = File::open(path)?;
        let chunker = StreamCDC::new(
            file,
            config.chunking.min_bytes,
            config.chunking.avg_bytes,
            config.chunking.max_bytes,
        );

        for (seq, chunk) in chunker.enumerate() {
            if let Some(cancel) = options.cancel
                && cancel.is_cancelled()
            {
                return Err(Error::Cancelled);
            }

            let chunk = chunk.map_err(|_| Error::InvalidConfig {
                message: "chunking failed".to_string(),
            })?;
            result.chunks_total += 1;
            result.bytes_read += chunk.data.len() as u64;

            let chunk_hash = blake3::hash(&chunk.data).to_hex().to_string();

            let exists = chunk_object_exists(&pool, provider, &chunk_hash).await?;
            if exists {
                result.bytes_deduped += chunk.data.len() as u64;
            } else {
                let encrypted =
                    encrypt_framed(&config.master_key, chunk_hash.as_bytes(), &chunk.data)?;
                let object_id = storage
                    .upload_document(&format!("chunk-{chunk_hash}.bin"), encrypted.clone())
                    .await?;

                sqlx::query(
                    r#"
                    INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
                    VALUES (?, ?, 'blake3', 'xchacha20poly1305', strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                    "#,
                )
                .bind(&chunk_hash)
                .bind(chunk.data.len() as i64)
                .execute(&pool)
                .await?;

                sqlx::query(
                    r#"
                    INSERT OR IGNORE INTO chunk_objects (chunk_hash, provider, object_id, created_at)
                    VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                    "#,
                )
                .bind(&chunk_hash)
                .bind(provider)
                .bind(&object_id)
                .execute(&pool)
                .await?;

                result.chunks_uploaded += 1;
                result.bytes_uploaded += encrypted.len() as u64;
            }

            sqlx::query(
                r#"
                INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len)
                VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(&file_id)
            .bind(seq as i64)
            .bind(&chunk_hash)
            .bind(chunk.offset as i64)
            .bind(chunk.length as i64)
            .execute(&pool)
            .await?;

            if let Some(sink) = options.progress {
                sink.on_progress(TaskProgress {
                    phase: "upload".to_string(),
                    files_total: None,
                    files_done: Some(result.files_indexed),
                    chunks_total: Some(result.chunks_total),
                    chunks_done: Some(result.chunks_total),
                    bytes_read: Some(result.bytes_read),
                    bytes_uploaded: Some(result.bytes_uploaded),
                    bytes_deduped: Some(result.bytes_deduped),
                });
            }
        }
    }

    if let Some(sink) = options.progress {
        sink.on_progress(TaskProgress {
            phase: "index".to_string(),
            files_total: None,
            files_done: Some(result.files_indexed),
            chunks_total: Some(result.chunks_total),
            chunks_done: Some(result.chunks_total),
            bytes_read: Some(result.bytes_read),
            bytes_uploaded: Some(result.bytes_uploaded),
            bytes_deduped: Some(result.bytes_deduped),
        });
    }

    let manifest = upload_index(&pool, storage, &config, &snapshot_id).await?;
    result.index_parts = manifest.parts.len() as u64;

    Ok(result)
}

async fn latest_snapshot_for_source(
    pool: &SqlitePool,
    source_path: &Path,
) -> Result<Option<String>> {
    let source = path_to_utf8(source_path)?;
    let row = sqlx::query(
        r#"
        SELECT snapshot_id
        FROM snapshots
        WHERE source_path = ?
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(source)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r| r.get::<String, _>("snapshot_id")))
}

async fn chunk_object_exists(pool: &SqlitePool, provider: &str, chunk_hash: &str) -> Result<bool> {
    let row: Option<SqliteRow> = sqlx::query(
        r#"
        SELECT 1 as present
        FROM chunk_objects
        WHERE provider = ? AND chunk_hash = ?
        LIMIT 1
        "#,
    )
    .bind(provider)
    .bind(chunk_hash)
    .fetch_optional(pool)
    .await?;
    Ok(row.is_some())
}

async fn upload_index<S: Storage>(
    pool: &SqlitePool,
    storage: &S,
    config: &BackupConfig,
    snapshot_id: &str,
) -> Result<IndexManifest> {
    // Ensure all file/chunk rows are committed, then read the DB file.
    // Note: remote_index_* tables are local cache and may be written after upload.
    let db_bytes = std::fs::read(&config.db_path)?;
    let compressed = zstd::stream::encode_all(db_bytes.as_slice(), 0)?;

    let mut parts = Vec::new();
    for (no, part_plain) in compressed.chunks(INDEX_PART_BYTES).enumerate() {
        let part_no = no as u32;
        let aad = index_part_aad(snapshot_id, part_no);
        let part_enc = encrypt_framed(&config.master_key, aad.as_bytes(), part_plain)?;
        let part_hash = blake3::hash(&part_enc).to_hex().to_string();
        let filename = format!("index-{snapshot_id}.sqlite.zst.enc.part-{part_no:06}.bin");
        let object_id = storage.upload_document(&filename, part_enc.clone()).await?;

        sqlx::query(
            r#"
            INSERT OR REPLACE INTO remote_index_parts (snapshot_id, part_no, provider, object_id, size, hash)
            VALUES (?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(snapshot_id)
        .bind(part_no as i64)
        .bind(storage.provider())
        .bind(&object_id)
        .bind(part_enc.len() as i64)
        .bind(&part_hash)
        .execute(pool)
        .await?;

        parts.push(IndexManifestPart {
            no: part_no,
            size: part_enc.len(),
            hash: part_hash,
            object_id,
        });
    }

    let manifest = IndexManifest {
        version: 1,
        snapshot_id: snapshot_id.to_string(),
        hash_alg: "blake3".to_string(),
        enc_alg: "xchacha20poly1305".to_string(),
        compression: "zstd".to_string(),
        parts,
    };
    let manifest_json = serde_json::to_vec(&manifest).map_err(|_| Error::InvalidConfig {
        message: "serialize index manifest failed".to_string(),
    })?;

    let manifest_enc = encrypt_framed(&config.master_key, snapshot_id.as_bytes(), &manifest_json)?;
    let manifest_filename = format!("index-{snapshot_id}.manifest.json.enc");
    let manifest_object_id = storage
        .upload_document(&manifest_filename, manifest_enc)
        .await?;

    sqlx::query(
        r#"
        INSERT OR REPLACE INTO remote_indexes (snapshot_id, provider, manifest_object_id, created_at)
        VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
        "#,
    )
    .bind(snapshot_id)
    .bind(storage.provider())
    .bind(&manifest_object_id)
    .execute(pool)
    .await?;

    Ok(manifest)
}

fn path_to_utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::NonUtf8Path {
            path: path.to_path_buf(),
        })
}
