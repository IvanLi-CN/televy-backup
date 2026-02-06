use std::collections::HashSet;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use sqlx::Row;
use walkdir::WalkDir;

use crate::{Error, Result};

/// A content-level comparison between a local folder and a remote snapshot index DB.
///
/// Design goals:
/// - Must NOT rely on any local index DB as a proof of equality.
/// - Uses the remote snapshot's file list + chunk hashes (BLAKE3) as the authority.
/// - Produces a small, UI-friendly summary (counts + a few example paths).
///
/// Notes / limitations:
/// - Only compares regular files (`kind == "file"`). Directories / symlinks are ignored because
///   restore currently only materializes regular files.
#[derive(Debug, Default, Clone)]
pub struct FolderCompareReport {
    pub remote_files_total: u64,
    pub local_files_total: u64,

    pub missing_local_files: u64,
    pub extra_local_files: u64,

    pub size_mismatch_files: u64,
    pub hash_mismatch_files: u64,
    pub io_error_files: u64,

    pub missing_local_examples: Vec<String>,
    pub extra_local_examples: Vec<String>,
    pub mismatch_examples: Vec<String>,
}

impl FolderCompareReport {
    pub fn is_match(&self) -> bool {
        self.missing_local_files == 0
            && self.extra_local_files == 0
            && self.size_mismatch_files == 0
            && self.hash_mismatch_files == 0
            && self.io_error_files == 0
    }
}

/// Compare a local directory against a remote snapshot index DB (SQLite).
///
/// - `index_db_path`: path to the downloaded remote index DB for `snapshot_id`
/// - `snapshot_id`: snapshot id to compare against
/// - `local_root`: local directory to compare
/// - `max_examples`: per-category example path cap (best-effort)
pub async fn compare_local_folder_against_index_db(
    index_db_path: &Path,
    snapshot_id: &str,
    local_root: &Path,
    max_examples: usize,
) -> Result<FolderCompareReport> {
    if !local_root.exists() {
        return Err(Error::InvalidConfig {
            message: format!("local root does not exist: {}", local_root.display()),
        });
    }
    if !local_root.is_dir() {
        return Err(Error::InvalidConfig {
            message: format!("local root is not a directory: {}", local_root.display()),
        });
    }

    let pool = crate::index_db::open_existing_index_db(index_db_path).await?;

    // Collect remote file list (regular files only).
    let remote_rows = sqlx::query(
        "SELECT file_id, path, size FROM files WHERE snapshot_id = ? AND kind = 'file' ORDER BY path",
    )
    .bind(snapshot_id)
    .fetch_all(&pool)
    .await?;

    let mut remote_files = HashSet::<String>::new();
    for row in &remote_rows {
        let path: String = row.get("path");
        remote_files.insert(path);
    }

    // Collect local file list (regular files only).
    let mut local_files = HashSet::<String>::new();
    for entry in WalkDir::new(local_root).follow_links(false) {
        let entry = entry.map_err(|e| Error::InvalidConfig {
            message: format!("walkdir error: {e}"),
        })?;
        if entry.path() == local_root {
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }

        let rel = entry.path().strip_prefix(local_root).map_err(|_| {
            Error::InvalidConfig {
                message: "strip_prefix failed".to_string(),
            }
        })?;
        let rel_str = rel.to_str().ok_or_else(|| Error::InvalidConfig {
            message: "local path is not valid utf-8".to_string(),
        })?;
        local_files.insert(rel_str.to_string());
    }

    let mut report = FolderCompareReport::default();
    report.remote_files_total = remote_files.len() as u64;
    report.local_files_total = local_files.len() as u64;

    // Fast mismatch checks: missing/extra files.
    for p in remote_files.iter() {
        if !local_files.contains(p) {
            report.missing_local_files += 1;
            if report.missing_local_examples.len() < max_examples {
                report.missing_local_examples.push(p.clone());
            }
        }
    }
    for p in local_files.iter() {
        if !remote_files.contains(p) {
            report.extra_local_files += 1;
            if report.extra_local_examples.len() < max_examples {
                report.extra_local_examples.push(p.clone());
            }
        }
    }

    if report.missing_local_files > 0 || report.extra_local_files > 0 {
        return Ok(report);
    }

    // Content-level verification using the remote snapshot's chunk hashes.
    // This reads local file bytes and computes BLAKE3 over each chunk defined by (offset, len).
    let mut buf = vec![0u8; 256 * 1024]; // reused read buffer (256KiB)

    for row in remote_rows {
        let file_id: String = row.get("file_id");
        let rel: String = row.get("path");
        let expected_size: i64 = row.get("size");

        let local_path = local_root.join(&rel);
        let meta = match std::fs::metadata(&local_path) {
            Ok(m) => m,
            Err(e) => {
                report.io_error_files += 1;
                if report.mismatch_examples.len() < max_examples {
                    report
                        .mismatch_examples
                        .push(format!("{rel} (stat failed: {e})"));
                }
                continue;
            }
        };
        if !meta.is_file() {
            report.io_error_files += 1;
            if report.mismatch_examples.len() < max_examples {
                report
                    .mismatch_examples
                    .push(format!("{rel} (not a regular file)"));
            }
            continue;
        }

        let actual_size = meta.len() as i64;
        if actual_size != expected_size {
            report.size_mismatch_files += 1;
            if report.mismatch_examples.len() < max_examples {
                report.mismatch_examples.push(format!(
                    "{rel} (size mismatch: expected={expected_size} got={actual_size})"
                ));
            }
            continue;
        }

        let mut f = match File::open(&local_path) {
            Ok(f) => f,
            Err(e) => {
                report.io_error_files += 1;
                if report.mismatch_examples.len() < max_examples {
                    report
                        .mismatch_examples
                        .push(format!("{rel} (open failed: {e})"));
                }
                continue;
            }
        };

        let chunks = sqlx::query(
            "SELECT seq, chunk_hash, offset, len FROM file_chunks WHERE file_id = ? ORDER BY seq",
        )
        .bind(&file_id)
        .fetch_all(&pool)
        .await?;

        let mut ok = true;
        for chunk_row in chunks {
            let expected_hash: String = chunk_row.get("chunk_hash");
            let offset: i64 = chunk_row.get("offset");
            let len: i64 = chunk_row.get("len");

            if offset < 0 || len < 0 {
                report.io_error_files += 1;
                if report.mismatch_examples.len() < max_examples {
                    report.mismatch_examples.push(format!(
                        "{rel} (invalid chunk offsets: offset={offset} len={len})"
                    ));
                }
                ok = false;
                break;
            }

            if let Err(e) = f.seek(SeekFrom::Start(offset as u64)) {
                report.io_error_files += 1;
                if report.mismatch_examples.len() < max_examples {
                    report
                        .mismatch_examples
                        .push(format!("{rel} (seek failed: {e})"));
                }
                ok = false;
                break;
            }

            let mut remaining = len as u64;
            let mut hasher = blake3::Hasher::new();
            while remaining > 0 {
                let want = std::cmp::min(remaining, buf.len() as u64) as usize;
                let n = match f.read(&mut buf[..want]) {
                    Ok(n) => n,
                    Err(e) => {
                        report.io_error_files += 1;
                        if report.mismatch_examples.len() < max_examples {
                            report
                                .mismatch_examples
                                .push(format!("{rel} (read failed: {e})"));
                        }
                        ok = false;
                        break;
                    }
                };
                if n == 0 {
                    report.io_error_files += 1;
                    if report.mismatch_examples.len() < max_examples {
                        report.mismatch_examples.push(format!(
                            "{rel} (unexpected EOF when reading chunk offset={offset} len={len})"
                        ));
                    }
                    ok = false;
                    break;
                }
                hasher.update(&buf[..n]);
                remaining = remaining.saturating_sub(n as u64);
            }
            if !ok {
                break;
            }

            let actual_hash = hasher.finalize().to_hex().to_string();
            if actual_hash != expected_hash {
                report.hash_mismatch_files += 1;
                if report.mismatch_examples.len() < max_examples {
                    report.mismatch_examples.push(format!(
                        "{rel} (chunk hash mismatch at offset={offset} len={len})"
                    ));
                }
                ok = false;
                break;
            }
        }

        if !ok {
            // Keep scanning other files to accumulate a few examples, but we already know mismatch.
            continue;
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::index_db::open_index_db;

    async fn init_test_db(path: &Path, snapshot_id: &str) -> sqlx::SqlitePool {
        let pool = open_index_db(path).await.unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, '2026-01-01T00:00:00Z', '/', 'manual', NULL)",
        )
        .bind(snapshot_id)
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    async fn insert_file_one_chunk(
        pool: &sqlx::SqlitePool,
        snapshot_id: &str,
        file_id: &str,
        rel: &str,
        bytes: &[u8],
        chunk_hash: &str,
    ) {
        sqlx::query(
            "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES (?, ?, ?, ?, 0, 0, 'file')",
        )
        .bind(file_id)
        .bind(snapshot_id)
        .bind(rel)
        .bind(bytes.len() as i64)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES (?, ?, 'blake3', 'xchacha20poly1305', '2026-01-01T00:00:00Z')",
        )
        .bind(chunk_hash)
        .bind(bytes.len() as i64)
        .execute(pool)
        .await
        .unwrap();

        sqlx::query(
            "INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len) VALUES (?, 0, ?, 0, ?)",
        )
        .bind(file_id)
        .bind(chunk_hash)
        .bind(bytes.len() as i64)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn compare_reports_match_when_identical() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("a.txt"), b"hello").unwrap();

        let db_path = dir.path().join("index.sqlite");
        let pool = init_test_db(&db_path, "s1").await;
        let hash = blake3::hash(b"hello").to_hex().to_string();
        insert_file_one_chunk(&pool, "s1", "f1", "a.txt", b"hello", &hash).await;

        let report =
            compare_local_folder_against_index_db(&db_path, "s1", &local, 10)
                .await
                .unwrap();
        assert!(report.is_match());
    }

    #[tokio::test]
    async fn compare_reports_missing_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        std::fs::create_dir_all(&local).unwrap();

        let db_path = dir.path().join("index.sqlite");
        let pool = init_test_db(&db_path, "s1").await;
        let hash = blake3::hash(b"hello").to_hex().to_string();
        insert_file_one_chunk(&pool, "s1", "f1", "a.txt", b"hello", &hash).await;

        let report =
            compare_local_folder_against_index_db(&db_path, "s1", &local, 10)
                .await
                .unwrap();
        assert!(!report.is_match());
        assert_eq!(report.missing_local_files, 1);
    }

    #[tokio::test]
    async fn compare_reports_extra_local_file() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("extra.txt"), b"x").unwrap();

        let db_path = dir.path().join("index.sqlite");
        init_test_db(&db_path, "s1").await;

        let report =
            compare_local_folder_against_index_db(&db_path, "s1", &local, 10)
                .await
                .unwrap();
        assert!(!report.is_match());
        assert_eq!(report.extra_local_files, 1);
    }

    #[tokio::test]
    async fn compare_reports_hash_mismatch() {
        let dir = tempfile::tempdir().unwrap();
        let local = dir.path().join("local");
        std::fs::create_dir_all(&local).unwrap();
        std::fs::write(local.join("a.txt"), b"hello").unwrap();

        let db_path = dir.path().join("index.sqlite");
        let pool = init_test_db(&db_path, "s1").await;
        let wrong = blake3::hash(b"world").to_hex().to_string();
        insert_file_one_chunk(&pool, "s1", "f1", "a.txt", b"hello", &wrong).await;

        let report =
            compare_local_folder_against_index_db(&db_path, "s1", &local, 10)
                .await
                .unwrap();
        assert!(!report.is_match());
        assert_eq!(report.hash_mismatch_files, 1);
    }
}

