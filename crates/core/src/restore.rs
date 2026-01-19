use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};

use crate::crypto::decrypt_framed;
use crate::index_db::open_existing_index_db;
use crate::index_manifest::{IndexManifest, index_part_aad};
use crate::progress::{ProgressSink, TaskProgress};
use crate::storage::Storage;
use crate::{Error, Result};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct RestoreConfig {
    pub snapshot_id: String,
    pub manifest_object_id: String,
    pub master_key: [u8; 32],
    pub index_db_path: PathBuf,
    pub target_path: PathBuf,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct RestoreResult {
    pub files_restored: u64,
    pub chunks_downloaded: u64,
    pub bytes_written: u64,
}

#[derive(Debug, Clone)]
pub struct VerifyConfig {
    pub snapshot_id: String,
    pub manifest_object_id: String,
    pub master_key: [u8; 32],
    pub index_db_path: PathBuf,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct VerifyResult {
    pub chunks_checked: u64,
    pub bytes_checked: u64,
}

pub async fn restore_snapshot<S: Storage>(
    storage: &S,
    config: RestoreConfig,
) -> Result<RestoreResult> {
    restore_snapshot_with(storage, config, RestoreOptions::default()).await
}

#[derive(Default)]
pub struct RestoreOptions<'a> {
    pub cancel: Option<&'a CancellationToken>,
    pub progress: Option<&'a dyn ProgressSink>,
}

pub async fn restore_snapshot_with<S: Storage>(
    storage: &S,
    config: RestoreConfig,
    options: RestoreOptions<'_>,
) -> Result<RestoreResult> {
    let _manifest = download_and_write_index_db(
        storage,
        &config.snapshot_id,
        &config.manifest_object_id,
        &config.master_key,
        &config.index_db_path,
        options.cancel,
    )
    .await?;

    ensure_empty_dir(&config.target_path)?;

    let pool = open_existing_index_db(&config.index_db_path).await?;
    ensure_snapshot_present(&pool, &config.snapshot_id).await?;

    restore_dirs(&pool, &config.snapshot_id, &config.target_path).await?;
    restore_files(
        storage,
        &pool,
        &config.snapshot_id,
        &config.target_path,
        &config.master_key,
        options.cancel,
        options.progress,
    )
    .await
}

pub async fn verify_snapshot<S: Storage>(
    storage: &S,
    config: VerifyConfig,
) -> Result<VerifyResult> {
    verify_snapshot_with(storage, config, VerifyOptions::default()).await
}

#[derive(Default)]
pub struct VerifyOptions<'a> {
    pub cancel: Option<&'a CancellationToken>,
    pub progress: Option<&'a dyn ProgressSink>,
}

pub async fn verify_snapshot_with<S: Storage>(
    storage: &S,
    config: VerifyConfig,
    options: VerifyOptions<'_>,
) -> Result<VerifyResult> {
    let _manifest = download_and_write_index_db(
        storage,
        &config.snapshot_id,
        &config.manifest_object_id,
        &config.master_key,
        &config.index_db_path,
        options.cancel,
    )
    .await?;

    let pool = open_existing_index_db(&config.index_db_path).await?;
    ensure_snapshot_present(&pool, &config.snapshot_id).await?;

    verify_chunks(
        storage,
        &pool,
        &config.snapshot_id,
        &config.master_key,
        options.cancel,
        options.progress,
    )
    .await
}

async fn download_and_write_index_db<S: Storage>(
    storage: &S,
    snapshot_id: &str,
    manifest_object_id: &str,
    master_key: &[u8; 32],
    index_db_path: &Path,
    cancel: Option<&CancellationToken>,
) -> Result<IndexManifest> {
    if let Some(cancel) = cancel
        && cancel.is_cancelled()
    {
        return Err(Error::Cancelled);
    }
    let manifest_enc = storage.download_document(manifest_object_id).await?;
    let manifest_json = decrypt_framed(master_key, snapshot_id.as_bytes(), &manifest_enc)?;

    let manifest: IndexManifest =
        serde_json::from_slice(&manifest_json).map_err(|e| Error::InvalidConfig {
            message: format!("invalid index manifest json: {e}"),
        })?;

    if manifest.version != 1 {
        return Err(Error::InvalidConfig {
            message: format!("unsupported manifest version: {}", manifest.version),
        });
    }
    if manifest.snapshot_id != snapshot_id {
        return Err(Error::InvalidConfig {
            message: "manifest snapshot_id mismatch".to_string(),
        });
    }
    if manifest.enc_alg != "xchacha20poly1305" {
        return Err(Error::InvalidConfig {
            message: format!("unsupported enc_alg: {}", manifest.enc_alg),
        });
    }
    if manifest.compression != "zstd" {
        return Err(Error::InvalidConfig {
            message: format!("unsupported compression: {}", manifest.compression),
        });
    }

    let mut parts = manifest.parts.clone();
    parts.sort_by_key(|p| p.no);

    let mut compressed = Vec::new();
    for part in parts {
        if let Some(cancel) = cancel
            && cancel.is_cancelled()
        {
            return Err(Error::Cancelled);
        }
        let part_enc = storage
            .download_document(&part.object_id)
            .await
            .map_err(|_| Error::MissingIndexPart {
                snapshot_id: snapshot_id.to_string(),
                part_no: part.no,
            })?;

        if part_enc.len() != part.size {
            return Err(Error::Integrity {
                message: format!(
                    "index part size mismatch: snapshot_id={snapshot_id} part_no={} expected={} got={}",
                    part.no,
                    part.size,
                    part_enc.len()
                ),
            });
        }

        let part_hash = blake3::hash(&part_enc).to_hex().to_string();
        if part_hash != part.hash {
            return Err(Error::Integrity {
                message: format!(
                    "index part hash mismatch: snapshot_id={snapshot_id} part_no={}",
                    part.no
                ),
            });
        }

        let aad = index_part_aad(snapshot_id, part.no);
        let part_plain = decrypt_framed(master_key, aad.as_bytes(), &part_enc)?;
        compressed.extend_from_slice(&part_plain);
    }

    let sqlite_bytes = zstd::stream::decode_all(compressed.as_slice())?;
    if let Some(parent) = index_db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(index_db_path, sqlite_bytes)?;
    Ok(manifest)
}

async fn ensure_snapshot_present(pool: &SqlitePool, snapshot_id: &str) -> Result<()> {
    let row = sqlx::query("SELECT 1 as present FROM snapshots WHERE snapshot_id = ? LIMIT 1")
        .bind(snapshot_id)
        .fetch_optional(pool)
        .await?;
    if row.is_none() {
        return Err(Error::InvalidConfig {
            message: format!("snapshot not found: {snapshot_id}"),
        });
    }
    Ok(())
}

async fn restore_dirs(pool: &SqlitePool, snapshot_id: &str, target: &Path) -> Result<()> {
    let rows =
        sqlx::query("SELECT path FROM files WHERE snapshot_id = ? AND kind = 'dir' ORDER BY path")
            .bind(snapshot_id)
            .fetch_all(pool)
            .await?;

    for row in rows {
        let rel: String = row.get("path");
        let path = target.join(rel);
        fs::create_dir_all(path)?;
    }

    Ok(())
}

async fn restore_files<S: Storage>(
    storage: &S,
    pool: &SqlitePool,
    snapshot_id: &str,
    target: &Path,
    master_key: &[u8; 32],
    cancel: Option<&CancellationToken>,
    progress: Option<&dyn ProgressSink>,
) -> Result<RestoreResult> {
    let mut result = RestoreResult::default();

    let rows = sqlx::query(
        "SELECT file_id, path, size, kind FROM files WHERE snapshot_id = ? ORDER BY path",
    )
    .bind(snapshot_id)
    .fetch_all(pool)
    .await?;

    for row in rows {
        if let Some(cancel) = cancel
            && cancel.is_cancelled()
        {
            return Err(Error::Cancelled);
        }

        let kind: String = row.get("kind");
        if kind != "file" {
            continue;
        }
        let file_id: String = row.get("file_id");
        let rel: String = row.get("path");
        let expected_size: i64 = row.get("size");

        let out_path = target.join(&rel);
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let mut out = fs::File::create(&out_path)?;

        let chunks = sqlx::query(
            "SELECT seq, chunk_hash, offset, len FROM file_chunks WHERE file_id = ? ORDER BY seq",
        )
        .bind(&file_id)
        .fetch_all(pool)
        .await?;

        for chunk_row in chunks {
            if let Some(cancel) = cancel
                && cancel.is_cancelled()
            {
                return Err(Error::Cancelled);
            }

            let chunk_hash: String = chunk_row.get("chunk_hash");
            let offset: i64 = chunk_row.get("offset");
            let len: i64 = chunk_row.get("len");

            let object_row = sqlx::query(
                "SELECT object_id FROM chunk_objects WHERE provider = ? AND chunk_hash = ? LIMIT 1",
            )
            .bind(storage.provider())
            .bind(&chunk_hash)
            .fetch_optional(pool)
            .await?;

            let object_id: String = match object_row {
                Some(r) => r.get("object_id"),
                None => {
                    return Err(Error::MissingChunkObject { chunk_hash });
                }
            };

            let framed = storage.download_document(&object_id).await.map_err(|_| {
                Error::MissingChunkObject {
                    chunk_hash: chunk_hash.clone(),
                }
            })?;
            let plain = decrypt_framed(master_key, chunk_hash.as_bytes(), &framed)?;

            let got_hash = blake3::hash(&plain).to_hex().to_string();
            if got_hash != chunk_hash {
                return Err(Error::Integrity {
                    message: format!("chunk hash mismatch: {chunk_hash}"),
                });
            }
            if plain.len() as i64 != len {
                return Err(Error::Integrity {
                    message: format!(
                        "chunk length mismatch: chunk_hash={chunk_hash} expected_len={len} got_len={}",
                        plain.len()
                    ),
                });
            }

            out.seek(SeekFrom::Start(offset as u64))?;
            out.write_all(&plain)?;

            result.chunks_downloaded += 1;
            result.bytes_written += plain.len() as u64;

            if let Some(sink) = progress {
                sink.on_progress(TaskProgress {
                    phase: "download".to_string(),
                    files_total: None,
                    files_done: Some(result.files_restored),
                    chunks_total: None,
                    chunks_done: Some(result.chunks_downloaded),
                    bytes_read: Some(result.bytes_written),
                    bytes_uploaded: None,
                    bytes_deduped: None,
                });
            }
        }

        out.flush()?;

        let written_size = fs::metadata(&out_path)?.len() as i64;
        if written_size != expected_size {
            return Err(Error::Integrity {
                message: format!(
                    "file size mismatch: path={rel} expected={expected_size} got={written_size}"
                ),
            });
        }

        result.files_restored += 1;

        if let Some(sink) = progress {
            sink.on_progress(TaskProgress {
                phase: "verify".to_string(),
                files_total: None,
                files_done: Some(result.files_restored),
                chunks_total: None,
                chunks_done: Some(result.chunks_downloaded),
                bytes_read: Some(result.bytes_written),
                bytes_uploaded: None,
                bytes_deduped: None,
            });
        }
    }

    Ok(result)
}

async fn verify_chunks<S: Storage>(
    storage: &S,
    pool: &SqlitePool,
    snapshot_id: &str,
    master_key: &[u8; 32],
    cancel: Option<&CancellationToken>,
    progress: Option<&dyn ProgressSink>,
) -> Result<VerifyResult> {
    let mut result = VerifyResult::default();

    let rows = sqlx::query(
        r#"
        SELECT DISTINCT fc.chunk_hash as chunk_hash
        FROM file_chunks fc
        JOIN files f ON f.file_id = fc.file_id
        WHERE f.snapshot_id = ?
        ORDER BY fc.chunk_hash
        "#,
    )
    .bind(snapshot_id)
    .fetch_all(pool)
    .await?;

    for row in rows {
        if let Some(cancel) = cancel
            && cancel.is_cancelled()
        {
            return Err(Error::Cancelled);
        }

        let chunk_hash: String = row.get("chunk_hash");
        let object_row = sqlx::query(
            "SELECT object_id FROM chunk_objects WHERE provider = ? AND chunk_hash = ? LIMIT 1",
        )
        .bind(storage.provider())
        .bind(&chunk_hash)
        .fetch_optional(pool)
        .await?;

        let object_id: String = match object_row {
            Some(r) => r.get("object_id"),
            None => {
                return Err(Error::MissingChunkObject { chunk_hash });
            }
        };

        let framed =
            storage
                .download_document(&object_id)
                .await
                .map_err(|_| Error::MissingChunkObject {
                    chunk_hash: chunk_hash.clone(),
                })?;
        let plain = decrypt_framed(master_key, chunk_hash.as_bytes(), &framed)?;

        let got_hash = blake3::hash(&plain).to_hex().to_string();
        if got_hash != chunk_hash {
            return Err(Error::Integrity {
                message: format!("chunk hash mismatch: {chunk_hash}"),
            });
        }

        result.chunks_checked += 1;
        result.bytes_checked += plain.len() as u64;

        if let Some(sink) = progress {
            sink.on_progress(TaskProgress {
                phase: "chunks".to_string(),
                files_total: None,
                files_done: None,
                chunks_total: None,
                chunks_done: Some(result.chunks_checked),
                bytes_read: Some(result.bytes_checked),
                bytes_uploaded: None,
                bytes_deduped: None,
            });
        }
    }

    Ok(result)
}

fn ensure_empty_dir(path: &Path) -> Result<()> {
    if path.exists() {
        let mut it = fs::read_dir(path)?;
        if it.next().transpose()?.is_some() {
            return Err(Error::InvalidConfig {
                message: "target_path must be an empty directory".to_string(),
            });
        }
        return Ok(());
    }
    fs::create_dir_all(path)?;
    Ok(())
}
