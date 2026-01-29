use std::fs;
use std::io::Write;
use std::path::Path;

use tokio_util::sync::CancellationToken;
use tracing::error;

use crate::crypto::decrypt_framed;
use crate::index_manifest::{IndexManifest, index_part_aad};
use crate::storage::Storage;
use crate::{Error, Result};

#[derive(Debug, Clone, Copy)]
pub struct DownloadedIndexDbStats {
    pub bytes_downloaded: u64,
    pub bytes_written: u64,
}

pub async fn download_and_write_index_db_atomic<S: Storage>(
    storage: &S,
    snapshot_id: &str,
    manifest_object_id: &str,
    master_key: &[u8; 32],
    index_db_path: &Path,
    cancel: Option<&CancellationToken>,
    normalize_provider: Option<&str>,
) -> Result<DownloadedIndexDbStats> {
    if let Some(cancel) = cancel
        && cancel.is_cancelled()
    {
        return Err(Error::Cancelled);
    }

    let manifest_enc = storage
        .download_document(manifest_object_id)
        .await
        .map_err(|e| {
            error!(
                event = "io.telegram.download_failed",
                snapshot_id,
                object_id = manifest_object_id,
                error = %e,
                "io.telegram.download_failed"
            );
            e
        })?;
    let mut bytes_downloaded = manifest_enc.len() as u64;

    let manifest_json = decrypt_framed(master_key, snapshot_id.as_bytes(), &manifest_enc).map_err(
        |e| Error::Crypto {
            message: format!(
                "manifest decrypt failed: snapshot_id={snapshot_id} object_id={manifest_object_id}; {e}"
            ),
        },
    )?;

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
            .map_err(|e| {
                error!(
                    event = "io.telegram.download_failed",
                    snapshot_id,
                    part_no = part.no,
                    object_id = %part.object_id,
                    error = %e,
                    "io.telegram.download_failed"
                );
                Error::MissingIndexPart {
                    snapshot_id: snapshot_id.to_string(),
                    part_no: part.no,
                }
            })?;

        bytes_downloaded = bytes_downloaded.saturating_add(part_enc.len() as u64);

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
        let part_plain = decrypt_framed(master_key, aad.as_bytes(), &part_enc).map_err(|e| {
            Error::Crypto {
                message: format!(
                    "index part decrypt failed: snapshot_id={snapshot_id} part_no={} object_id={}; {e}",
                    part.no, part.object_id
                ),
            }
        })?;
        compressed.extend_from_slice(&part_plain);
    }

    let sqlite_bytes = zstd::stream::decode_all(compressed.as_slice())?;
    let bytes_written = sqlite_bytes.len() as u64;

    write_index_db_atomic(index_db_path, &sqlite_bytes, normalize_provider).await?;

    Ok(DownloadedIndexDbStats {
        bytes_downloaded,
        bytes_written,
    })
}

async fn write_index_db_atomic(
    path: &Path,
    bytes: &[u8],
    normalize_provider: Option<&str>,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut tmp = path.to_path_buf();
    tmp.set_extension(format!("tmp-{}", uuid::Uuid::new_v4()));

    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }

    #[cfg(not(unix))]
    {
        fs::write(&tmp, bytes)?;
    }

    if let Some(provider) = normalize_provider {
        normalize_provider_in_index_db(&tmp, provider).await?;
    }

    replace_atomic(&tmp, path)?;
    Ok(())
}

async fn normalize_provider_in_index_db(path: &Path, provider: &str) -> Result<()> {
    let kind = provider_kind(provider);
    let like = format!("{kind}%");

    let pool = crate::index_db::open_existing_index_db(path).await?;
    // `chunk_objects` has unique constraints involving `provider`, so avoid hard failures when the
    // downloaded DB already contains the normalized provider (e.g. two endpoint IDs pointing to the
    // same chat). The duplicates are harmless for lookups.
    sqlx::query(
        "UPDATE OR IGNORE chunk_objects SET provider = ? WHERE provider != ? AND provider LIKE ?",
    )
    .bind(provider)
    .bind(provider)
    .bind(&like)
    .execute(&pool)
    .await?;
    sqlx::query("UPDATE remote_indexes SET provider = ? WHERE provider != ? AND provider LIKE ?")
        .bind(provider)
        .bind(provider)
        .bind(&like)
        .execute(&pool)
        .await?;
    sqlx::query(
        "UPDATE remote_index_parts SET provider = ? WHERE provider != ? AND provider LIKE ?",
    )
    .bind(provider)
    .bind(provider)
    .bind(&like)
    .execute(&pool)
    .await?;

    Ok(())
}

fn provider_kind(provider: &str) -> &str {
    provider.split(['/', ':']).next().unwrap_or(provider).trim()
}

fn replace_atomic(tmp: &Path, path: &Path) -> std::io::Result<()> {
    match fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Some platforms (e.g. Windows) do not allow renaming over an existing destination.
            // Avoid deleting the existing DB: move it aside, then restore it if the replace fails.
            let mut backup = path.to_path_buf();
            backup.set_extension(format!("bak-{}", uuid::Uuid::new_v4()));

            fs::rename(path, &backup)?;
            match fs::rename(tmp, path) {
                Ok(()) => {
                    let _ = fs::remove_file(&backup);
                    Ok(())
                }
                Err(e) => {
                    let _ = fs::rename(&backup, path);
                    let _ = fs::remove_file(tmp);
                    Err(e)
                }
            }
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::Row;

    #[tokio::test]
    async fn download_and_write_roundtrip_writes_expected_sqlite_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let source_db = dir.path().join("source.sqlite");
        let out_db = dir.path().join("out.sqlite");

        let pool = crate::index_db::open_index_db(&source_db).await.unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, '2026-01-01T00:00:00Z', '/', 'manual', NULL)",
        )
        .bind("snp_1")
        .execute(&pool)
        .await
        .unwrap();

        drop(pool);

        let sqlite_bytes = fs::read(&source_db).unwrap();
        let compressed = zstd::stream::encode_all(sqlite_bytes.as_slice(), 0).unwrap();

        let snapshot_id = "snp_1";
        let master_key = [9u8; 32];

        let part_no = 0u32;
        let aad = index_part_aad(snapshot_id, part_no);
        let part_enc =
            crate::crypto::encrypt_framed(&master_key, aad.as_bytes(), &compressed).unwrap();
        let part_hash = blake3::hash(&part_enc).to_hex().to_string();

        let storage = crate::InMemoryStorage::new();
        let part_object_id = storage.upload_document("part.dat", part_enc).await.unwrap();

        let manifest = IndexManifest {
            version: 1,
            snapshot_id: snapshot_id.to_string(),
            hash_alg: "blake3".to_string(),
            enc_alg: "xchacha20poly1305".to_string(),
            compression: "zstd".to_string(),
            parts: vec![crate::index_manifest::IndexManifestPart {
                no: part_no,
                size: storage
                    .download_document(&part_object_id)
                    .await
                    .unwrap()
                    .len(),
                hash: part_hash,
                object_id: part_object_id,
            }],
        };
        let manifest_json = serde_json::to_vec(&manifest).unwrap();
        let manifest_enc =
            crate::crypto::encrypt_framed(&master_key, snapshot_id.as_bytes(), &manifest_json)
                .unwrap();
        let manifest_object_id = storage
            .upload_document("manifest.dat", manifest_enc)
            .await
            .unwrap();

        let stats = download_and_write_index_db_atomic(
            &storage,
            snapshot_id,
            &manifest_object_id,
            &master_key,
            &out_db,
            None,
            None,
        )
        .await
        .unwrap();

        assert!(stats.bytes_downloaded > 0);
        assert_eq!(stats.bytes_written as usize, sqlite_bytes.len());

        let out_bytes = fs::read(&out_db).unwrap();
        assert_eq!(out_bytes, sqlite_bytes);
    }

    #[tokio::test]
    async fn download_and_write_normalizes_provider() {
        let dir = tempfile::tempdir().unwrap();
        let source_db = dir.path().join("source.sqlite");
        let out_db = dir.path().join("out.sqlite");

        let pool = crate::index_db::open_index_db(&source_db).await.unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, '2026-01-01T00:00:00Z', '/', 'manual', NULL)",
        )
        .bind("snp_1")
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES ('chk_1', 1, 'blake3', 'xchacha20poly1305', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at) VALUES ('chk_1', 'telegram.mtproto/old', 'obj_1', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO remote_indexes (snapshot_id, provider, manifest_object_id, created_at) VALUES ('snp_1', 'telegram.mtproto/old', 'man_1', '2026-01-01T00:00:00Z')",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO remote_index_parts (snapshot_id, part_no, provider, object_id, size, hash) VALUES ('snp_1', 0, 'telegram.mtproto/old', 'part_1', 1, 'h')",
        )
        .execute(&pool)
        .await
        .unwrap();

        drop(pool);

        let sqlite_bytes = fs::read(&source_db).unwrap();
        let compressed = zstd::stream::encode_all(sqlite_bytes.as_slice(), 0).unwrap();

        let snapshot_id = "snp_1";
        let master_key = [9u8; 32];

        let part_no = 0u32;
        let aad = index_part_aad(snapshot_id, part_no);
        let part_enc =
            crate::crypto::encrypt_framed(&master_key, aad.as_bytes(), &compressed).unwrap();
        let part_hash = blake3::hash(&part_enc).to_hex().to_string();

        let storage = crate::InMemoryStorage::new();
        let part_object_id = storage.upload_document("part.dat", part_enc).await.unwrap();

        let manifest = IndexManifest {
            version: 1,
            snapshot_id: snapshot_id.to_string(),
            hash_alg: "blake3".to_string(),
            enc_alg: "xchacha20poly1305".to_string(),
            compression: "zstd".to_string(),
            parts: vec![crate::index_manifest::IndexManifestPart {
                no: part_no,
                size: storage
                    .download_document(&part_object_id)
                    .await
                    .unwrap()
                    .len(),
                hash: part_hash,
                object_id: part_object_id,
            }],
        };
        let manifest_json = serde_json::to_vec(&manifest).unwrap();
        let manifest_enc =
            crate::crypto::encrypt_framed(&master_key, snapshot_id.as_bytes(), &manifest_json)
                .unwrap();
        let manifest_object_id = storage
            .upload_document("manifest.dat", manifest_enc)
            .await
            .unwrap();

        download_and_write_index_db_atomic(
            &storage,
            snapshot_id,
            &manifest_object_id,
            &master_key,
            &out_db,
            None,
            Some("telegram.mtproto/new"),
        )
        .await
        .unwrap();

        let pool = crate::index_db::open_existing_index_db(&out_db)
            .await
            .unwrap();
        let row = sqlx::query("SELECT provider FROM chunk_objects LIMIT 1")
            .fetch_one(&pool)
            .await
            .unwrap();
        let provider: String = row.try_get("provider").unwrap();
        assert_eq!(provider, "telegram.mtproto/new");

        let row = sqlx::query("SELECT provider FROM remote_indexes WHERE snapshot_id = 'snp_1'")
            .fetch_one(&pool)
            .await
            .unwrap();
        let provider: String = row.try_get("provider").unwrap();
        assert_eq!(provider, "telegram.mtproto/new");

        let row = sqlx::query(
            "SELECT provider FROM remote_index_parts WHERE snapshot_id = 'snp_1' AND part_no = 0",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        let provider: String = row.try_get("provider").unwrap();
        assert_eq!(provider, "telegram.mtproto/new");
    }
}
