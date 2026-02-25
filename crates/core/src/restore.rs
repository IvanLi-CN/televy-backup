use std::fs;
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use serde::{Deserialize, Serialize};
use sqlx::{Row, SqlitePool};
use tracing::{debug, error};

use crate::crypto::decrypt_framed;
use crate::index_db::open_existing_index_db;
use crate::pack::extract_pack_blob;
use crate::progress::{ProgressSink, TaskProgress};
use crate::remote_index_db::download_and_write_index_db_atomic;
use crate::storage::{ChunkObjectRef, Storage, parse_chunk_object_ref};
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
    let restore_started = Instant::now();
    debug!(event = "phase.start", phase = "restore", "phase.start");

    let stats = download_and_write_index_db_atomic(
        storage,
        &config.snapshot_id,
        &config.manifest_object_id,
        &config.master_key,
        &config.index_db_path,
        options.cancel,
        Some(storage.provider()),
        options.progress,
    )
    .await?;

    let mut bytes_downloaded = stats.bytes_downloaded;
    let mut net_bytes_downloaded = stats.net_bytes_downloaded.unwrap_or(0);
    let have_net_bytes_downloaded = Arc::new(AtomicBool::new(stats.net_bytes_downloaded.is_some()));

    ensure_empty_dir(&config.target_path)?;

    let pool = open_existing_index_db(&config.index_db_path).await?;
    ensure_snapshot_present(&pool, &config.snapshot_id).await?;

    restore_dirs(&pool, &config.snapshot_id, &config.target_path).await?;
    let result = restore_files(
        storage,
        &pool,
        &config.snapshot_id,
        &config.target_path,
        &config.master_key,
        options.cancel,
        &mut bytes_downloaded,
        &mut net_bytes_downloaded,
        Arc::clone(&have_net_bytes_downloaded),
        options.progress,
    )
    .await?;

    debug!(
        event = "phase.finish",
        phase = "restore",
        duration_ms = restore_started.elapsed().as_millis() as u64,
        files_restored = result.files_restored,
        chunks_downloaded = result.chunks_downloaded,
        bytes_written = result.bytes_written,
        "phase.finish"
    );

    Ok(result)
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
    let verify_started = Instant::now();
    debug!(event = "phase.start", phase = "verify", "phase.start");

    let stats = download_and_write_index_db_atomic(
        storage,
        &config.snapshot_id,
        &config.manifest_object_id,
        &config.master_key,
        &config.index_db_path,
        options.cancel,
        Some(storage.provider()),
        options.progress,
    )
    .await?;

    let mut bytes_downloaded = stats.bytes_downloaded;
    let mut net_bytes_downloaded = stats.net_bytes_downloaded.unwrap_or(0);
    let have_net_bytes_downloaded = Arc::new(AtomicBool::new(stats.net_bytes_downloaded.is_some()));

    let pool = open_existing_index_db(&config.index_db_path).await?;
    ensure_snapshot_present(&pool, &config.snapshot_id).await?;

    let result = verify_chunks(
        storage,
        &pool,
        &config.snapshot_id,
        &config.master_key,
        options.cancel,
        &mut bytes_downloaded,
        &mut net_bytes_downloaded,
        Arc::clone(&have_net_bytes_downloaded),
        options.progress,
    )
    .await?;

    debug!(
        event = "phase.finish",
        phase = "verify",
        duration_ms = verify_started.elapsed().as_millis() as u64,
        chunks_checked = result.chunks_checked,
        bytes_checked = result.bytes_checked,
        "phase.finish"
    );

    Ok(result)
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

#[allow(clippy::too_many_arguments)]
async fn restore_files<S: Storage>(
    storage: &S,
    pool: &SqlitePool,
    snapshot_id: &str,
    target: &Path,
    master_key: &[u8; 32],
    cancel: Option<&CancellationToken>,
    bytes_downloaded: &mut u64,
    net_bytes_downloaded: &mut u64,
    have_net_bytes_downloaded: Arc<AtomicBool>,
    progress: Option<&dyn ProgressSink>,
) -> Result<RestoreResult> {
    let mut result = RestoreResult::default();
    let mut pack_cache: Option<(String, Vec<u8>)> = None;

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
            r#"
            SELECT fc.seq, fc.chunk_hash, fc.offset, fc.len, co.object_id as object_id
            FROM file_chunks fc
            LEFT JOIN chunk_objects co
              ON co.chunk_hash = fc.chunk_hash
             AND co.provider = ?
            WHERE fc.file_id = ?
            ORDER BY fc.seq
            "#,
        )
        .bind(storage.provider())
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
            let encoded_object_id: Option<String> = chunk_row.get("object_id");
            let encoded_object_id = encoded_object_id.ok_or_else(|| Error::MissingChunkObject {
                chunk_hash: chunk_hash.clone(),
            })?;
            let object_ref = parse_chunk_object_ref(&encoded_object_id)?;

            let plain = match object_ref {
                ChunkObjectRef::Direct { object_id } => {
                    let base_total = *bytes_downloaded;
                    let base_net_total = *net_bytes_downloaded;
                    // Use a sentinel so "0 bytes downloaded" (e.g. fully satisfied from cache) is
                    // distinguishable from "no progress callbacks were ever emitted".
                    let latest = Arc::new(AtomicU64::new(u64::MAX));
                    let latest_for_cb = latest.clone();
                    let latest_net = Arc::new(AtomicU64::new(u64::MAX));
                    let latest_net_for_cb = latest_net.clone();
                    let have_net_for_cb = Arc::clone(&have_net_bytes_downloaded);
                    let framed = storage
                        .download_document_with_progress(
                            &object_id,
                            Some(Box::new(move |p| {
                                let n = p.bytes;
                                latest_for_cb.store(n, Ordering::Relaxed);
                                if let Some(net) = p.net_bytes {
                                    latest_net_for_cb.store(net, Ordering::Relaxed);
                                    have_net_for_cb.store(true, Ordering::Relaxed);
                                }
                                if let Some(sink) = progress {
                                    sink.on_progress(TaskProgress {
                                        phase: "download".to_string(),
                                        bytes_downloaded: Some(base_total.saturating_add(n)),
                                        net_bytes_downloaded: p
                                            .net_bytes
                                            .map(|net| base_net_total.saturating_add(net)),
                                        ..TaskProgress::default()
                                    });
                                }
                            })),
                        )
                        .await
                        .map_err(|e| {
                            error!(
                                    event = "io.telegram.download_failed",
                                    snapshot_id,
                                    object_id = %object_id,
                                    chunk_hash,
                                error = %e,
                                "io.telegram.download_failed"
                            );
                            match e {
                                Error::Telegram { message } => {
                                    // Treat "not found" style errors as permanent missing data, but keep
                                    // timeouts/transient failures as retryable telegram errors.
                                    if message.contains("message not found")
                                        || message.contains("document mismatch")
                                    {
                                        Error::MissingChunkObject {
                                            chunk_hash: chunk_hash.clone(),
                                        }
                                    } else {
                                        Error::Telegram { message }
                                    }
                                }
                                _other => Error::MissingChunkObject {
                                    chunk_hash: chunk_hash.clone(),
                                },
                            }
                        })?;
                    let streamed = latest.load(Ordering::Relaxed);
                    let actual = if streamed != u64::MAX {
                        streamed
                    } else {
                        framed.len() as u64
                    };
                    *bytes_downloaded = base_total.saturating_add(actual);
                    let streamed_net = latest_net.load(Ordering::Relaxed);
                    if streamed_net != u64::MAX {
                        *net_bytes_downloaded = base_net_total.saturating_add(streamed_net);
                    }
                    decrypt_framed(master_key, chunk_hash.as_bytes(), &framed).map_err(|e| {
                        Error::Crypto {
                            message: format!(
                                "chunk decrypt failed: snapshot_id={snapshot_id} chunk_hash={chunk_hash} object_id={object_id}; {e}"
                            ),
                        }
                    })?
                }
                ChunkObjectRef::PackSlice {
                    pack_object_id,
                    offset: pack_off,
                    len: pack_len,
                } => {
                    let pack_bytes = match &pack_cache {
                        Some((cached_id, cached_bytes)) if cached_id == &pack_object_id => {
                            cached_bytes.as_slice()
                        }
                        _ => {
                            let base_total = *bytes_downloaded;
                            let base_net_total = *net_bytes_downloaded;
                            // Use a sentinel so "0 bytes downloaded" (e.g. fully satisfied from
                            // cache) is distinguishable from "no progress callbacks were ever
                            // emitted".
                            let latest = Arc::new(AtomicU64::new(u64::MAX));
                            let latest_for_cb = latest.clone();
                            let latest_net = Arc::new(AtomicU64::new(u64::MAX));
                            let latest_net_for_cb = latest_net.clone();
                            let have_net_for_cb = Arc::clone(&have_net_bytes_downloaded);
                            let bytes = storage
                                .download_document_with_progress(
                                    &pack_object_id,
                                    Some(Box::new(move |p| {
                                        let n = p.bytes;
                                        latest_for_cb.store(n, Ordering::Relaxed);
                                        if let Some(net) = p.net_bytes {
                                            latest_net_for_cb.store(net, Ordering::Relaxed);
                                            have_net_for_cb.store(true, Ordering::Relaxed);
                                        }
                                        if let Some(sink) = progress {
                                            sink.on_progress(TaskProgress {
                                                phase: "download".to_string(),
                                                bytes_downloaded: Some(
                                                    base_total.saturating_add(n),
                                                ),
                                                net_bytes_downloaded: p
                                                    .net_bytes
                                                    .map(|net| base_net_total.saturating_add(net)),
                                                ..TaskProgress::default()
                                            });
                                        }
                                    })),
                                )
                                .await
                                .map_err(|e| {
                                    error!(
                                        event = "io.telegram.download_failed",
                                        snapshot_id,
                                        object_id = %pack_object_id,
                                        chunk_hash,
                                        error = %e,
                                        "io.telegram.download_failed"
                                    );
                                    match e {
                                        Error::Telegram { message } => {
                                            if message.contains("message not found")
                                                || message.contains("document mismatch")
                                            {
                                                Error::MissingChunkObject {
                                                    chunk_hash: chunk_hash.clone(),
                                                }
                                            } else {
                                                Error::Telegram { message }
                                            }
                                        }
                                        _other => Error::MissingChunkObject {
                                            chunk_hash: chunk_hash.clone(),
                                        },
                                    }
                                })?;
                            let streamed = latest.load(Ordering::Relaxed);
                            let actual = if streamed != u64::MAX {
                                streamed
                            } else {
                                bytes.len() as u64
                            };
                            *bytes_downloaded = base_total.saturating_add(actual);
                            let streamed_net = latest_net.load(Ordering::Relaxed);
                            if streamed_net != u64::MAX {
                                *net_bytes_downloaded = base_net_total.saturating_add(streamed_net);
                            }
                            pack_cache = Some((pack_object_id.clone(), bytes));
                            pack_cache.as_ref().expect("just set").1.as_slice()
                        }
                    };

                    if pack_len > usize::MAX as u64 {
                        return Err(Error::Integrity {
                            message: "pack slice too large".to_string(),
                        });
                    }
                    let framed = extract_pack_blob(pack_bytes, pack_off, pack_len)?;
                    decrypt_framed(master_key, chunk_hash.as_bytes(), framed).map_err(|e| {
                        Error::Crypto {
                            message: format!(
                                "chunk decrypt failed (pack slice): snapshot_id={snapshot_id} chunk_hash={chunk_hash} pack_object_id={pack_object_id} offset={pack_off} len={pack_len}; {e}"
                            ),
                        }
                    })?
                }
            };

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
                    source_files_total: None,
                    source_bytes_total: None,
                    files_total: None,
                    files_done: Some(result.files_restored),
                    chunks_total: None,
                    chunks_done: Some(result.chunks_downloaded),
                    bytes_read: Some(result.bytes_written),
                    bytes_uploaded: None,
                    net_bytes_uploaded: None,
                    bytes_downloaded: Some(*bytes_downloaded),
                    net_bytes_downloaded: have_net_bytes_downloaded
                        .load(Ordering::Relaxed)
                        .then_some(*net_bytes_downloaded),
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
                phase: "restore".to_string(),
                source_files_total: None,
                source_bytes_total: None,
                files_total: None,
                files_done: Some(result.files_restored),
                chunks_total: None,
                chunks_done: Some(result.chunks_downloaded),
                bytes_read: Some(result.bytes_written),
                bytes_uploaded: None,
                net_bytes_uploaded: None,
                bytes_downloaded: Some(*bytes_downloaded),
                net_bytes_downloaded: have_net_bytes_downloaded
                    .load(Ordering::Relaxed)
                    .then_some(*net_bytes_downloaded),
                bytes_deduped: None,
            });
        }
    }

    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn verify_chunks<S: Storage>(
    storage: &S,
    pool: &SqlitePool,
    snapshot_id: &str,
    master_key: &[u8; 32],
    cancel: Option<&CancellationToken>,
    bytes_downloaded: &mut u64,
    net_bytes_downloaded: &mut u64,
    have_net_bytes_downloaded: Arc<AtomicBool>,
    progress: Option<&dyn ProgressSink>,
) -> Result<VerifyResult> {
    let mut result = VerifyResult::default();
    let mut pack_cache: Option<(String, Vec<u8>)> = None;

    let rows = sqlx::query(
        r#"
        SELECT co.chunk_hash as chunk_hash, co.object_id as object_id
        FROM chunk_objects co
        JOIN (
          SELECT DISTINCT fc.chunk_hash as chunk_hash
          FROM file_chunks fc
          JOIN files f ON f.file_id = fc.file_id
          WHERE f.snapshot_id = ?
        ) used ON used.chunk_hash = co.chunk_hash
        WHERE co.provider = ?
        ORDER BY co.object_id, co.chunk_hash
        "#,
    )
    .bind(snapshot_id)
    .bind(storage.provider())
    .fetch_all(pool)
    .await?;

    for row in rows {
        if let Some(cancel) = cancel
            && cancel.is_cancelled()
        {
            return Err(Error::Cancelled);
        }

        let chunk_hash: String = row.get("chunk_hash");
        let encoded_object_id: String = row.get("object_id");
        let object_ref = parse_chunk_object_ref(&encoded_object_id)?;

        let plain = match object_ref {
            ChunkObjectRef::Direct { object_id } => {
                let base_total = *bytes_downloaded;
                let base_net_total = *net_bytes_downloaded;
                // Use a sentinel so "0 bytes downloaded" (e.g. fully satisfied from cache) is
                // distinguishable from "no progress callbacks were ever emitted".
                let latest = Arc::new(AtomicU64::new(u64::MAX));
                let latest_for_cb = latest.clone();
                let latest_net = Arc::new(AtomicU64::new(u64::MAX));
                let latest_net_for_cb = latest_net.clone();
                let have_net_for_cb = Arc::clone(&have_net_bytes_downloaded);
                let framed = storage
                    .download_document_with_progress(
                        &object_id,
                        Some(Box::new(move |p| {
                            let n = p.bytes;
                            latest_for_cb.store(n, Ordering::Relaxed);
                            if let Some(net) = p.net_bytes {
                                latest_net_for_cb.store(net, Ordering::Relaxed);
                                have_net_for_cb.store(true, Ordering::Relaxed);
                            }
                            if let Some(sink) = progress {
                                sink.on_progress(TaskProgress {
                                    phase: "chunks".to_string(),
                                    bytes_downloaded: Some(base_total.saturating_add(n)),
                                    net_bytes_downloaded: p
                                        .net_bytes
                                        .map(|net| base_net_total.saturating_add(net)),
                                    ..TaskProgress::default()
                                });
                            }
                        })),
                    )
                    .await
                    .map_err(|e| {
                        error!(
                            event = "io.telegram.download_failed",
                            snapshot_id,
                            object_id = %object_id,
                            chunk_hash,
                            error = %e,
                            "io.telegram.download_failed"
                        );
                        match e {
                            Error::Telegram { message } => {
                                if message.contains("message not found")
                                    || message.contains("document mismatch")
                                {
                                    Error::MissingChunkObject {
                                        chunk_hash: chunk_hash.clone(),
                                    }
                                } else {
                                    Error::Telegram { message }
                                }
                            }
                            _other => Error::MissingChunkObject {
                                chunk_hash: chunk_hash.clone(),
                            },
                        }
                    })?;
                let streamed = latest.load(Ordering::Relaxed);
                let actual = if streamed != u64::MAX {
                    streamed
                } else {
                    framed.len() as u64
                };
                *bytes_downloaded = base_total.saturating_add(actual);
                let streamed_net = latest_net.load(Ordering::Relaxed);
                if streamed_net != u64::MAX {
                    *net_bytes_downloaded = base_net_total.saturating_add(streamed_net);
                }
                decrypt_framed(master_key, chunk_hash.as_bytes(), &framed).map_err(|e| {
                    Error::Crypto {
                        message: format!(
                            "chunk decrypt failed: snapshot_id={snapshot_id} chunk_hash={chunk_hash} object_id={object_id}; {e}"
                        ),
                    }
                })?
            }
            ChunkObjectRef::PackSlice {
                pack_object_id,
                offset: pack_off,
                len: pack_len,
            } => {
                let pack_bytes = match &pack_cache {
                    Some((cached_id, cached_bytes)) if cached_id == &pack_object_id => {
                        cached_bytes.as_slice()
                    }
                    _ => {
                        let base_total = *bytes_downloaded;
                        let base_net_total = *net_bytes_downloaded;
                        // Use a sentinel so "0 bytes downloaded" (e.g. fully satisfied from cache)
                        // is distinguishable from "no progress callbacks were ever emitted".
                        let latest = Arc::new(AtomicU64::new(u64::MAX));
                        let latest_for_cb = latest.clone();
                        let latest_net = Arc::new(AtomicU64::new(u64::MAX));
                        let latest_net_for_cb = latest_net.clone();
                        let have_net_for_cb = Arc::clone(&have_net_bytes_downloaded);
                        let bytes = storage
                            .download_document_with_progress(
                                &pack_object_id,
                                Some(Box::new(move |p| {
                                    let n = p.bytes;
                                    latest_for_cb.store(n, Ordering::Relaxed);
                                    if let Some(net) = p.net_bytes {
                                        latest_net_for_cb.store(net, Ordering::Relaxed);
                                        have_net_for_cb.store(true, Ordering::Relaxed);
                                    }
                                    if let Some(sink) = progress {
                                        sink.on_progress(TaskProgress {
                                            phase: "chunks".to_string(),
                                            bytes_downloaded: Some(base_total.saturating_add(n)),
                                            net_bytes_downloaded: p
                                                .net_bytes
                                                .map(|net| base_net_total.saturating_add(net)),
                                            ..TaskProgress::default()
                                        });
                                    }
                                })),
                            )
                            .await
                            .map_err(|e| {
                                error!(
                                event = "io.telegram.download_failed",
                                snapshot_id,
                                object_id = %pack_object_id,
                                chunk_hash,
                                    error = %e,
                                    "io.telegram.download_failed"
                                );
                                match e {
                                    Error::Telegram { message } => {
                                        if message.contains("message not found")
                                            || message.contains("document mismatch")
                                        {
                                            Error::MissingChunkObject {
                                                chunk_hash: chunk_hash.clone(),
                                            }
                                        } else {
                                            Error::Telegram { message }
                                        }
                                    }
                                    _other => Error::MissingChunkObject {
                                        chunk_hash: chunk_hash.clone(),
                                    },
                                }
                            })?;
                        let streamed = latest.load(Ordering::Relaxed);
                        let actual = if streamed != u64::MAX {
                            streamed
                        } else {
                            bytes.len() as u64
                        };
                        *bytes_downloaded = base_total.saturating_add(actual);
                        let streamed_net = latest_net.load(Ordering::Relaxed);
                        if streamed_net != u64::MAX {
                            *net_bytes_downloaded = base_net_total.saturating_add(streamed_net);
                        }
                        pack_cache = Some((pack_object_id.clone(), bytes));
                        pack_cache.as_ref().expect("just set").1.as_slice()
                    }
                };

                if pack_len > usize::MAX as u64 {
                    return Err(Error::Integrity {
                        message: "pack slice too large".to_string(),
                    });
                }
                let framed = extract_pack_blob(pack_bytes, pack_off, pack_len)?;
                decrypt_framed(master_key, chunk_hash.as_bytes(), framed).map_err(|e| {
                    Error::Crypto {
                        message: format!(
                            "chunk decrypt failed (pack slice): snapshot_id={snapshot_id} chunk_hash={chunk_hash} pack_object_id={pack_object_id} offset={pack_off} len={pack_len}; {e}"
                        ),
                    }
                })?
            }
        };

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
                source_files_total: None,
                source_bytes_total: None,
                files_total: None,
                files_done: None,
                chunks_total: None,
                chunks_done: Some(result.chunks_checked),
                bytes_read: Some(result.bytes_checked),
                bytes_uploaded: None,
                net_bytes_uploaded: None,
                bytes_downloaded: Some(*bytes_downloaded),
                net_bytes_downloaded: have_net_bytes_downloaded
                    .load(Ordering::Relaxed)
                    .then_some(*net_bytes_downloaded),
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
