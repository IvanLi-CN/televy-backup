//! Read-only access primitives used by the Finder snapshot browsing volume.
//!
//! This module deliberately stops at the verified logical-file boundary. The daemon owns
//! sessions and HTTP/WebDAV policy; core owns snapshot metadata, chunk resolution, decryption,
//! and integrity verification.

use std::cmp::{max, min};
use std::fs::OpenOptions;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use chrono::{DateTime, Local, Utc};
use serde::Serialize;
use sqlx::{Row, SqlitePool};
use tokio::sync::Mutex;

use crate::crypto::decrypt_framed;
use crate::index_db::open_readonly_index_db;
use crate::pack::extract_pack_blob;
use crate::storage::{ChunkObjectRef, Storage, parse_chunk_object_ref};
use crate::{Error, Result};

pub const DIAGNOSTICS_DIRECTORY: &str = "TelevyBackup Diagnostics";
pub const UNAVAILABLE_ENTRIES_FILE: &str = "Unavailable Entries.json";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowseSnapshot {
    pub snapshot_id: String,
    pub display_name: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BrowseEntry {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub mode: i64,
}

#[derive(Clone)]
pub struct SnapshotContentReader {
    endpoint_db_path: PathBuf,
    filemap_dir: PathBuf,
    dedupe_db_path: Option<PathBuf>,
    storage_provider: String,
    storage: Option<Arc<dyn Storage + Send + Sync>>,
    master_key: Option<[u8; 32]>,
    cache: Arc<SnapshotBrowseCache>,
}

impl SnapshotContentReader {
    pub fn new(
        endpoint_db_path: impl Into<PathBuf>,
        filemap_dir: impl Into<PathBuf>,
        storage: Arc<dyn Storage + Send + Sync>,
        master_key: [u8; 32],
        cache: Arc<SnapshotBrowseCache>,
    ) -> Self {
        Self {
            endpoint_db_path: endpoint_db_path.into(),
            filemap_dir: filemap_dir.into(),
            dedupe_db_path: None,
            storage_provider: storage.provider().to_string(),
            storage: Some(storage),
            master_key: Some(master_key),
            cache,
        }
    }

    /// Creates a reader that can inspect retained local metadata without initializing remote
    /// storage or loading the vault. File reads remain fail-closed until both a decryption key
    /// and a storage provider are available (or a previously fetched object is cached).
    pub fn new_cached(
        endpoint_db_path: impl Into<PathBuf>,
        filemap_dir: impl Into<PathBuf>,
        storage_provider: impl Into<String>,
        cache: Arc<SnapshotBrowseCache>,
    ) -> Self {
        Self {
            endpoint_db_path: endpoint_db_path.into(),
            filemap_dir: filemap_dir.into(),
            dedupe_db_path: None,
            storage_provider: storage_provider.into(),
            storage: None,
            master_key: None,
            cache,
        }
    }

    pub fn with_dedupe_db(mut self, path: impl Into<PathBuf>) -> Self {
        self.dedupe_db_path = Some(path.into());
        self
    }

    pub async fn list_snapshots(&self, source_path: &str) -> Result<Vec<BrowseSnapshot>> {
        let pool = open_readonly_index_db(&self.endpoint_db_path).await?;
        let rows = sqlx::query(
            "SELECT snapshot_id, created_at FROM snapshots WHERE source_path = ? ORDER BY created_at DESC, snapshot_id DESC",
        )
        .bind(source_path)
        .fetch_all(&pool)
        .await?;

        let mut snapshots = Vec::with_capacity(rows.len());
        let mut names = std::collections::HashSet::new();
        for row in rows {
            let snapshot_id: String = row.get("snapshot_id");
            let created_at: String = row.get("created_at");
            let base = local_display_time(&created_at);
            let short_id = snapshot_id.chars().take(8).collect::<String>();
            let mut display_name = format!("{base} [{short_id}]");
            if !names.insert(display_name.clone()) {
                display_name = format!(
                    "{base} [{short_id}-{}]",
                    snapshot_id.chars().skip(8).take(4).collect::<String>()
                );
                let mut suffix = 2;
                while !names.insert(display_name.clone()) {
                    display_name = format!("{base} [{short_id}-{suffix}]");
                    suffix += 1;
                }
            }
            snapshots.push(BrowseSnapshot {
                snapshot_id,
                display_name,
                created_at,
            });
        }
        Ok(snapshots)
    }

    pub async fn entry(
        &self,
        snapshot_id: &str,
        relative_path: &str,
    ) -> Result<Option<BrowseEntry>> {
        validate_relative_path(relative_path)?;
        if relative_path.is_empty() {
            return Ok(Some(BrowseEntry {
                path: String::new(),
                name: String::new(),
                kind: "dir".to_string(),
                size: 0,
                mtime_ms: 0,
                mode: 0o755,
            }));
        }
        let (pool, _, _) = self.filemap_pool(snapshot_id).await?;
        let row = sqlx::query(
            "SELECT path, size, mtime_ms, mode, kind FROM files WHERE snapshot_id = ? AND path = ? LIMIT 1",
        )
        .bind(snapshot_id)
        .bind(relative_path)
        .fetch_optional(&pool)
        .await?;
        Ok(row.map(|row| {
            let path: String = row.get("path");
            BrowseEntry {
                name: path.rsplit('/').next().unwrap_or(&path).to_string(),
                path,
                kind: row.get("kind"),
                size: non_negative_u64(row.get::<i64, _>("size")),
                mtime_ms: row.get("mtime_ms"),
                mode: row.get("mode"),
            }
        }))
    }

    pub async fn list_children(&self, snapshot_id: &str, parent: &str) -> Result<Vec<BrowseEntry>> {
        validate_relative_path(parent)?;
        let (pool, _, _) = self.filemap_pool(snapshot_id).await?;
        let rows = sqlx::query(
            "SELECT path, size, mtime_ms, mode, kind FROM files WHERE snapshot_id = ? ORDER BY path",
        )
        .bind(snapshot_id)
        .fetch_all(&pool)
        .await?;
        let prefix = if parent.is_empty() {
            String::new()
        } else {
            format!("{parent}/")
        };
        let mut entries = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for row in rows {
            let path: String = row.get("path");
            let Some(rest) = path.strip_prefix(&prefix) else {
                continue;
            };
            if rest.is_empty() {
                continue;
            }
            let name = rest.split('/').next().unwrap_or(rest);
            if !seen.insert(name.to_string()) {
                continue;
            }
            let is_direct = !rest.contains('/');
            let kind: String = row.get("kind");
            let kind = if is_direct { kind } else { "dir".to_string() };
            entries.push(BrowseEntry {
                path: format!("{prefix}{name}"),
                name: name.to_string(),
                kind,
                size: if is_direct {
                    non_negative_u64(row.get::<i64, _>("size"))
                } else {
                    0
                },
                mtime_ms: if is_direct { row.get("mtime_ms") } else { 0 },
                mode: if is_direct { row.get("mode") } else { 0o755 },
            });
        }
        Ok(entries)
    }

    pub async fn unavailable_entries(&self, snapshot_id: &str) -> Result<Vec<String>> {
        let (pool, _, _) = self.filemap_pool(snapshot_id).await?;
        let rows = sqlx::query(
            "SELECT path FROM files WHERE snapshot_id = ? AND kind NOT IN ('file', 'dir') ORDER BY path",
        )
        .bind(snapshot_id)
        .fetch_all(&pool)
        .await?;
        Ok(rows.into_iter().map(|row| row.get("path")).collect())
    }

    pub async fn read_range(
        &self,
        snapshot_id: &str,
        relative_path: &str,
        start: u64,
        requested_len: Option<u64>,
    ) -> Result<(BrowseEntry, Vec<u8>)> {
        let entry = self
            .entry(snapshot_id, relative_path)
            .await?
            .ok_or_else(|| Error::SnapshotAccess {
                message: "file not found".to_string(),
            })?;
        if entry.kind != "file" {
            return Err(Error::SnapshotAccess {
                message: "path is not a regular file".to_string(),
            });
        }
        if start > entry.size {
            return Err(Error::SnapshotAccess {
                message: "range starts beyond file size".to_string(),
            });
        }
        let end = requested_len
            .map(|len| start.saturating_add(len))
            .unwrap_or(entry.size)
            .min(entry.size);
        if end <= start {
            return Ok((entry, Vec::new()));
        }
        let (pool, endpoint_attached, dedupe_attached) = self.filemap_pool(snapshot_id).await?;
        let file_id: String = sqlx::query_scalar(
            "SELECT file_id FROM files WHERE snapshot_id = ? AND path = ? AND kind = 'file' LIMIT 1",
        )
        .bind(snapshot_id)
        .bind(relative_path)
        .fetch_one(&pool)
        .await?;
        let rows = match (endpoint_attached, dedupe_attached) {
            (true, true) => {
                sqlx::query(
                    "SELECT fc.chunk_hash, fc.offset, fc.len,
                            COALESCE(ep.object_id, dd.object_id, co.object_id) AS object_id
                     FROM file_chunks fc
                     LEFT JOIN browse_endpoint.chunk_objects ep
                       ON ep.chunk_hash = fc.chunk_hash AND ep.provider = ?
                     LEFT JOIN browse_dedupe.chunk_objects dd
                       ON dd.chunk_hash = fc.chunk_hash AND dd.provider = ?
                     LEFT JOIN chunk_objects co
                       ON co.chunk_hash = fc.chunk_hash AND co.provider = ?
                     WHERE fc.file_id = ? ORDER BY fc.seq",
                )
                .bind(&self.storage_provider)
                .bind(&self.storage_provider)
                .bind(&self.storage_provider)
                .bind(&file_id)
                .fetch_all(&pool)
                .await?
            }
            (true, false) => {
                sqlx::query(
                    "SELECT fc.chunk_hash, fc.offset, fc.len,
                            COALESCE(ep.object_id, co.object_id) AS object_id
                     FROM file_chunks fc
                     LEFT JOIN browse_endpoint.chunk_objects ep
                       ON ep.chunk_hash = fc.chunk_hash AND ep.provider = ?
                     LEFT JOIN chunk_objects co
                       ON co.chunk_hash = fc.chunk_hash AND co.provider = ?
                     WHERE fc.file_id = ? ORDER BY fc.seq",
                )
                .bind(&self.storage_provider)
                .bind(&self.storage_provider)
                .bind(&file_id)
                .fetch_all(&pool)
                .await?
            }
            (false, true) => {
                sqlx::query(
                    "SELECT fc.chunk_hash, fc.offset, fc.len,
                            COALESCE(dd.object_id, co.object_id) AS object_id
                     FROM file_chunks fc
                     LEFT JOIN browse_dedupe.chunk_objects dd
                       ON dd.chunk_hash = fc.chunk_hash AND dd.provider = ?
                     LEFT JOIN chunk_objects co
                       ON co.chunk_hash = fc.chunk_hash AND co.provider = ?
                     WHERE fc.file_id = ? ORDER BY fc.seq",
                )
                .bind(&self.storage_provider)
                .bind(&self.storage_provider)
                .bind(&file_id)
                .fetch_all(&pool)
                .await?
            }
            (false, false) => {
                sqlx::query(
                    "SELECT fc.chunk_hash, fc.offset, fc.len, co.object_id AS object_id
                     FROM file_chunks fc
                     LEFT JOIN chunk_objects co
                       ON co.chunk_hash = fc.chunk_hash AND co.provider = ?
                     WHERE fc.file_id = ? ORDER BY fc.seq",
                )
                .bind(&self.storage_provider)
                .bind(&file_id)
                .fetch_all(&pool)
                .await?
            }
        };

        let mut out = Vec::with_capacity((end - start) as usize);
        for row in rows {
            let chunk_offset = non_negative_u64(row.get::<i64, _>("offset"));
            let chunk_len = non_negative_u64(row.get::<i64, _>("len"));
            let chunk_end = chunk_offset.saturating_add(chunk_len);
            if chunk_end <= start || chunk_offset >= end {
                continue;
            }
            let chunk_hash: String = row.get("chunk_hash");
            let encoded_object_id: Option<String> = row.get("object_id");
            let encoded_object_id = encoded_object_id.ok_or_else(|| Error::MissingChunkObject {
                chunk_hash: chunk_hash.clone(),
            })?;
            let plain = self.load_chunk(&chunk_hash, &encoded_object_id).await?;
            if plain.len() as u64 != chunk_len {
                return Err(Error::Integrity {
                    message: format!("chunk length mismatch: {chunk_hash}"),
                });
            }
            let copy_start = max(start, chunk_offset) - chunk_offset;
            let copy_end = min(end, chunk_end) - chunk_offset;
            out.extend_from_slice(&plain[copy_start as usize..copy_end as usize]);
        }
        if out.len() as u64 != end - start {
            return Err(Error::Integrity {
                message: "file range has missing chunks".to_string(),
            });
        }
        Ok((entry, out))
    }

    async fn load_chunk(&self, chunk_hash: &str, encoded_object_id: &str) -> Result<Vec<u8>> {
        let master_key = self.master_key.ok_or_else(|| Error::SnapshotAccess {
            message: "snapshot content decryption is unavailable while backup storage is offline"
                .to_string(),
        })?;
        match parse_chunk_object_ref(encoded_object_id)? {
            ChunkObjectRef::Direct { object_id } => {
                let key = format!("{}:{object_id}", self.storage_provider);
                let framed = self.load_object(&key, &object_id).await?;
                let plain = decrypt_framed(&master_key, chunk_hash.as_bytes(), &framed).map_err(
                    |error| Error::Crypto {
                        message: format!("chunk decrypt failed: {error}"),
                    },
                )?;
                verify_chunk(chunk_hash, plain)
            }
            ChunkObjectRef::PackSlice {
                pack_object_id,
                offset,
                len,
            } => {
                let key = format!("{}:{pack_object_id}", self.storage_provider);
                let pack = self.load_object(&key, &pack_object_id).await?;
                let framed = extract_pack_blob(&pack, offset, len)?;
                let plain = decrypt_framed(&master_key, chunk_hash.as_bytes(), framed).map_err(
                    |error| Error::Crypto {
                        message: format!("pack chunk decrypt failed: {error}"),
                    },
                )?;
                verify_chunk(chunk_hash, plain)
            }
        }
    }

    async fn load_object(&self, key: &str, object_id: &str) -> Result<Vec<u8>> {
        if let Some(storage) = &self.storage {
            return self
                .cache
                .get_or_fetch(key, storage.download_document(object_id))
                .await;
        }
        self.cache
            .get(key)
            .await?
            .ok_or_else(|| Error::SnapshotAccess {
                message: "snapshot content is unavailable while backup storage is offline"
                    .to_string(),
            })
    }

    async fn filemap_pool(&self, snapshot_id: &str) -> Result<(SqlitePool, bool, bool)> {
        let filemap = self.filemap_dir.join(format!("{snapshot_id}.sqlite"));
        let path = if filemap.is_file() {
            filemap
        } else if endpoint_has_snapshot_files(&self.endpoint_db_path, snapshot_id).await? {
            self.endpoint_db_path.clone()
        } else {
            return Err(Error::SnapshotAccess {
                message: "snapshot filemap is not available locally".to_string(),
            });
        };
        let pool = open_readonly_index_db(&path).await?;
        if path != self.endpoint_db_path {
            sqlx::query("ATTACH DATABASE ? AS browse_endpoint")
                .bind(self.endpoint_db_path.to_string_lossy().to_string())
                .execute(&pool)
                .await?;
        }
        let dedupe_attached = self
            .dedupe_db_path
            .as_ref()
            .is_some_and(|path| path.is_file());
        if dedupe_attached {
            sqlx::query("ATTACH DATABASE ? AS browse_dedupe")
                .bind(
                    self.dedupe_db_path
                        .as_ref()
                        .expect("dedupe path checked above")
                        .to_string_lossy()
                        .to_string(),
                )
                .execute(&pool)
                .await?;
        }
        Ok((pool, path != self.endpoint_db_path, dedupe_attached))
    }
}

async fn endpoint_has_snapshot_files(endpoint_db_path: &Path, snapshot_id: &str) -> Result<bool> {
    let pool = open_readonly_index_db(endpoint_db_path).await?;
    Ok(
        sqlx::query("SELECT 1 FROM files WHERE snapshot_id = ? LIMIT 1")
            .bind(snapshot_id)
            .fetch_optional(&pool)
            .await?
            .is_some(),
    )
}

fn verify_chunk(chunk_hash: &str, plain: Vec<u8>) -> Result<Vec<u8>> {
    if blake3::hash(&plain).to_hex().to_string() != chunk_hash {
        return Err(Error::Integrity {
            message: format!("chunk hash mismatch: {chunk_hash}"),
        });
    }
    Ok(plain)
}

fn validate_relative_path(path: &str) -> Result<()> {
    if path.is_empty() {
        return Ok(());
    }
    if path.starts_with('/')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(Error::SnapshotAccess {
            message: "invalid snapshot-relative path".to_string(),
        });
    }
    Ok(())
}

fn non_negative_u64(value: i64) -> u64 {
    value.max(0) as u64
}

fn local_display_time(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|dt| {
            dt.with_timezone(&Local)
                .format("%Y-%m-%d %H-%M-%S")
                .to_string()
        })
        .or_else(|_| {
            value.parse::<DateTime<Utc>>().map(|dt| {
                dt.with_timezone(&Local)
                    .format("%Y-%m-%d %H-%M-%S")
                    .to_string()
            })
        })
        .unwrap_or_else(|_| {
            value
                .replace([':', 'T', 'Z'], "-")
                .chars()
                .take(19)
                .collect()
        })
}

#[derive(Debug)]
pub struct SnapshotBrowseCache {
    root: PathBuf,
    max_bytes: u64,
    lock: Mutex<()>,
}

impl SnapshotBrowseCache {
    pub fn new(root: impl Into<PathBuf>, max_bytes: u64) -> Self {
        Self {
            root: root.into(),
            max_bytes: max_bytes.max(1),
            lock: Mutex::new(()),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn get_or_fetch<F>(&self, key: &str, fetch: F) -> Result<Vec<u8>>
    where
        F: std::future::Future<Output = Result<Vec<u8>>>,
    {
        let _guard = self.lock.lock().await;
        tokio::fs::create_dir_all(&self.root).await?;
        restrict_directory_permissions(&self.root)?;
        let path = self.path_for(key);
        if path.is_file() {
            let bytes = tokio::fs::read(&path).await?;
            touch_file(&path)?;
            return Ok(bytes);
        }
        let bytes = fetch.await?;
        if bytes.len() as u64 > self.max_bytes {
            return Err(Error::SnapshotAccess {
                message: "encrypted object exceeds snapshot browse cache quota".to_string(),
            });
        }
        self.evict_for(bytes.len() as u64, None).await?;
        let temp = self.root.join(format!(".{}.tmp", uuid::Uuid::new_v4()));
        tokio::fs::write(&temp, &bytes).await?;
        restrict_file_permissions(&temp)?;
        tokio::fs::rename(&temp, &path).await?;
        Ok(bytes)
    }

    pub async fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let _guard = self.lock.lock().await;
        let path = self.path_for(key);
        if !path.is_file() {
            return Ok(None);
        }
        let bytes = tokio::fs::read(&path).await?;
        touch_file(&path)?;
        Ok(Some(bytes))
    }

    async fn evict_for(&self, incoming: u64, keep: Option<&Path>) -> Result<()> {
        let mut files = Vec::new();
        let mut total = 0u64;
        let mut dir = tokio::fs::read_dir(&self.root).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if !path.is_file() || path.extension().and_then(|ext| ext.to_str()) == Some("tmp") {
                continue;
            }
            let metadata = entry.metadata().await?;
            let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
            total = total.saturating_add(metadata.len());
            files.push((modified, path, metadata.len()));
        }
        files.sort_by_key(|(modified, _, _)| *modified);
        for (_, path, size) in files {
            if total.saturating_add(incoming) <= self.max_bytes {
                break;
            }
            if keep.is_some_and(|keep| keep == path) {
                continue;
            }
            tokio::fs::remove_file(path).await?;
            total = total.saturating_sub(size);
        }
        if total.saturating_add(incoming) > self.max_bytes {
            return Err(Error::SnapshotAccess {
                message: "snapshot browse cache quota exhausted".to_string(),
            });
        }
        Ok(())
    }

    fn path_for(&self, key: &str) -> PathBuf {
        let digest = blake3::hash(key.as_bytes()).to_hex().to_string();
        self.root.join(format!("{digest}.object"))
    }
}

fn touch_file(path: &Path) -> Result<()> {
    let file = OpenOptions::new().append(true).open(path)?;
    file.set_modified(SystemTime::now())?;
    Ok(())
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(unix)]
fn restrict_file_permissions(path: &Path) -> Result<()> {
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    Ok(())
}

#[cfg(not(unix))]
fn restrict_file_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_names_have_no_timezone_suffix() {
        let value = local_display_time("2026-09-11T14:05:37+08:00");
        assert_eq!(value.len(), 19);
        assert!(!value.contains('+'));
    }

    #[test]
    fn relative_paths_reject_traversal() {
        assert!(validate_relative_path("../secret").is_err());
        assert!(validate_relative_path("a/../secret").is_err());
        assert!(validate_relative_path("a/file").is_ok());
    }

    #[tokio::test]
    async fn cache_writes_raw_objects_and_enforces_quota() {
        let directory = tempfile::tempdir().expect("temp cache");
        let cache = SnapshotBrowseCache::new(directory.path(), 4);
        let first = cache
            .get_or_fetch("first", async { Ok::<_, Error>(vec![0, 1, 2]) })
            .await
            .expect("first object");
        assert_eq!(first, vec![0, 1, 2]);
        let cached_path = cache.path_for("first");
        assert_eq!(
            tokio::fs::read(cached_path).await.expect("cached bytes"),
            first
        );

        let too_large = cache
            .get_or_fetch("too-large", async { Ok::<_, Error>(vec![3, 4, 5, 6, 7]) })
            .await;
        assert!(too_large.is_err());
    }

    #[tokio::test]
    async fn cached_reader_lists_filemap_without_storage() {
        let temp = tempfile::tempdir().unwrap();
        let endpoint_db = temp.path().join("index.sqlite");
        let endpoint_pool = crate::index_db::open_index_db(&endpoint_db).await.unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, ?, ?, ?, NULL)",
        )
        .bind("snapshot-1")
        .bind("2026-09-11T08:00:00Z")
        .bind("/source")
        .bind("Test")
        .execute(&endpoint_pool)
        .await
        .unwrap();
        drop(endpoint_pool);

        let filemap_dir = temp.path().join("filemaps");
        std::fs::create_dir_all(&filemap_dir).unwrap();
        let filemap = filemap_dir.join("snapshot-1.sqlite");
        let filemap_pool = crate::index_db::open_snapshot_filemap_db(&filemap)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, ?, ?, ?, NULL)",
        )
        .bind("snapshot-1")
        .bind("2026-09-11T08:00:00Z")
        .bind("/source")
        .bind("Test")
        .execute(&filemap_pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind("folder-1")
        .bind("snapshot-1")
        .bind("Folder")
        .bind(0i64)
        .bind(0i64)
        .bind(0o755i64)
        .bind("dir")
        .execute(&filemap_pool)
        .await
        .unwrap();
        drop(filemap_pool);

        let reader = SnapshotContentReader::new_cached(
            endpoint_db,
            filemap_dir,
            "telegram.mtproto/default",
            Arc::new(SnapshotBrowseCache::new(temp.path().join("cache"), 1024)),
        );
        let snapshots = reader.list_snapshots("/source").await.unwrap();
        assert_eq!(snapshots.len(), 1);
        assert_eq!(
            reader.list_children("snapshot-1", "").await.unwrap()[0].name,
            "Folder"
        );
    }
}
