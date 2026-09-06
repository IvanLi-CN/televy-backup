use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use base64::Engine;
use futures::TryStreamExt;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};

use crate::storage::{ChunkObjectRef, parse_chunk_object_ref, storage_object_id};
use crate::{Error, index_db};

pub const MAX_PAGE_SIZE: u16 = 500;
const CURSOR_VERSION: u8 = 1;
const STORAGE_INDEX_SCHEMA_VERSION: i64 = 1;
const STORAGE_INDEX_BUILD_BATCH_SIZE: usize = 256;
const STORAGE_INDEX_TEMP_MAX_AGE: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, thiserror::Error)]
pub enum SnapshotInspectionError {
    #[error("snapshot was not found: {snapshot_id}")]
    SnapshotNotFound { snapshot_id: String },
    #[error("snapshot is no longer retained: {snapshot_id}")]
    SnapshotNotRetained { snapshot_id: String },
    #[error("snapshot filemap is unavailable: {snapshot_id}; {message}")]
    FilemapUnavailable {
        snapshot_id: String,
        message: String,
    },
    #[error("the direct baseline is unavailable: {snapshot_id}")]
    BaselineUnavailable { snapshot_id: String },
    #[error("invalid snapshot inspection argument: {message}")]
    InvalidArgument { message: String },
    #[error("invalid snapshot inspection cursor: {message}")]
    InvalidCursor { message: String },
    #[error("storage inspection index is not ready: {snapshot_id}")]
    StorageIndexUnavailable { snapshot_id: String },
    #[error(transparent)]
    Core(#[from] Error),
}

pub type Result<T> = std::result::Result<T, SnapshotInspectionError>;

impl From<sqlx::Error> for SnapshotInspectionError {
    fn from(value: sqlx::Error) -> Self {
        Self::Core(Error::Sqlite(value))
    }
}

#[derive(Debug, Clone)]
pub struct SnapshotInspector {
    endpoint_db_path: PathBuf,
    filemap_dir: PathBuf,
    storage_db_path: PathBuf,
    storage_index_dir: PathBuf,
}

impl SnapshotInspector {
    pub fn new(endpoint_db_path: impl Into<PathBuf>, filemap_dir: impl Into<PathBuf>) -> Self {
        let endpoint_db_path = endpoint_db_path.into();
        let filemap_dir = filemap_dir.into();
        Self {
            endpoint_db_path: endpoint_db_path.clone(),
            storage_index_dir: default_storage_index_dir(&filemap_dir),
            filemap_dir,
            storage_db_path: endpoint_db_path,
        }
    }

    pub fn new_with_storage_db(
        endpoint_db_path: impl Into<PathBuf>,
        filemap_dir: impl Into<PathBuf>,
        storage_db_path: impl Into<PathBuf>,
    ) -> Self {
        let filemap_dir = filemap_dir.into();
        Self {
            endpoint_db_path: endpoint_db_path.into(),
            storage_index_dir: default_storage_index_dir(&filemap_dir),
            filemap_dir,
            storage_db_path: storage_db_path.into(),
        }
    }

    pub fn new_with_storage_db_and_index_dir(
        endpoint_db_path: impl Into<PathBuf>,
        filemap_dir: impl Into<PathBuf>,
        storage_db_path: impl Into<PathBuf>,
        storage_index_dir: impl Into<PathBuf>,
    ) -> Self {
        Self {
            endpoint_db_path: endpoint_db_path.into(),
            filemap_dir: filemap_dir.into(),
            storage_db_path: storage_db_path.into(),
            storage_index_dir: storage_index_dir.into(),
        }
    }

    pub fn endpoint_db_path(&self) -> &Path {
        &self.endpoint_db_path
    }

    pub fn filemap_path(&self, snapshot_id: &str) -> PathBuf {
        self.filemap_dir.join(format!("{snapshot_id}.sqlite"))
    }

    pub fn storage_index_path(&self, snapshot_id: &str) -> PathBuf {
        self.storage_index_dir.join(format!("{snapshot_id}.sqlite"))
    }

    pub async fn summary(&self, snapshot_id: &str) -> Result<SnapshotSummary> {
        let context = self.resolve_context(snapshot_id).await?;
        let pool = index_db::open_readonly_index_db(&context.current_path).await?;
        let mut connection = pool.acquire().await?;
        let attached = attach_baseline_if_needed(&mut connection, &context).await?;

        let file_row = sqlx::query(
            r#"
            SELECT
              COUNT(*) AS entries,
              COALESCE(SUM(CASE WHEN kind = 'file' THEN 1 ELSE 0 END), 0) AS regular_files,
              COALESCE(SUM(CASE WHEN kind = 'dir' THEN 1 ELSE 0 END), 0) AS directories,
              COALESCE(SUM(CASE WHEN kind = 'symlink' THEN 1 ELSE 0 END), 0) AS symlinks,
              COALESCE(SUM(CASE WHEN kind = 'file' THEN size ELSE 0 END), 0) AS bytes
            FROM files
            WHERE snapshot_id = ?
            "#,
        )
        .bind(snapshot_id)
        .fetch_one(&mut *connection)
        .await?;

        let block_row = sqlx::query(
            r#"
            SELECT COUNT(*) AS distinct_blocks, COALESCE(SUM(size), 0) AS bytes
            FROM (
              SELECT fc.chunk_hash, MAX(c.size) AS size
              FROM file_chunks fc
              JOIN files f ON f.file_id = fc.file_id
              JOIN chunks c ON c.chunk_hash = fc.chunk_hash
              WHERE f.snapshot_id = ? AND f.kind = 'file'
              GROUP BY fc.chunk_hash
            )
            "#,
        )
        .bind(snapshot_id)
        .fetch_one(&mut *connection)
        .await?;

        let files = FileCounts {
            entries: non_negative_u64(&file_row, "entries"),
            regular_files: non_negative_u64(&file_row, "regular_files"),
            directories: non_negative_u64(&file_row, "directories"),
            symlinks: non_negative_u64(&file_row, "symlinks"),
            bytes: non_negative_u64(&file_row, "bytes"),
        };
        let blocks = BlockCounts {
            distinct: non_negative_u64(&block_row, "distinct_blocks"),
            bytes: non_negative_u64(&block_row, "bytes"),
        };

        let changes = match &context.difference {
            DifferenceContext::FirstSnapshot => ChangeSummary {
                state: "firstSnapshot".to_string(),
                added: files.entries,
                deleted: 0,
                changed: 0,
            },
            DifferenceContext::BaselineUnavailable => ChangeSummary {
                state: "baselineUnavailable".to_string(),
                added: 0,
                deleted: 0,
                changed: 0,
            },
            DifferenceContext::Available { snapshot_id, .. } => {
                let cte = changes_cte(context.base_table_name());
                let sql = format!(
                    "{cte} SELECT \
                       COALESCE(SUM(CASE WHEN change = 'added' THEN 1 ELSE 0 END), 0) AS added, \
                       COALESCE(SUM(CASE WHEN change = 'deleted' THEN 1 ELSE 0 END), 0) AS deleted, \
                       COALESCE(SUM(CASE WHEN change = 'changed' THEN 1 ELSE 0 END), 0) AS changed \
                     FROM changes"
                );
                let row = bind_changes_query(
                    sqlx::query(&sql),
                    snapshot_id,
                    &context.metadata.snapshot_id,
                    &context.metadata.snapshot_id,
                    snapshot_id,
                )
                .fetch_one(&mut *connection)
                .await?;
                ChangeSummary {
                    state: "available".to_string(),
                    added: non_negative_u64(&row, "added"),
                    deleted: non_negative_u64(&row, "deleted"),
                    changed: non_negative_u64(&row, "changed"),
                }
            }
        };

        detach_baseline(&mut connection, attached).await?;

        Ok(SnapshotSummary {
            snapshot: context.metadata,
            availability: DifferenceAvailability::from_context(&context.difference),
            files,
            changes,
            blocks,
        })
    }

    /// Prepares one retained snapshot for repeated local file-tree inspection.
    ///
    /// The prepared state contains only filemap metadata and direct-baseline
    /// classifications. It is intended for a long-lived local consumer such as
    /// the daemon, which can serve many expanded tree nodes without rerunning
    /// the full SQL difference query for every node.
    pub async fn prepare(&self, snapshot_id: &str) -> Result<SnapshotInspectionSession> {
        let context = self.resolve_context(snapshot_id).await?;
        let pool = index_db::open_readonly_index_db(&context.current_path).await?;
        let mut connection = pool.acquire().await?;
        let attached = attach_baseline_if_needed(&mut connection, &context).await?;

        let file_row = sqlx::query(
            r#"
            SELECT
              COUNT(*) AS entries,
              COALESCE(SUM(CASE WHEN kind = 'file' THEN 1 ELSE 0 END), 0) AS regular_files,
              COALESCE(SUM(CASE WHEN kind = 'dir' THEN 1 ELSE 0 END), 0) AS directories,
              COALESCE(SUM(CASE WHEN kind = 'symlink' THEN 1 ELSE 0 END), 0) AS symlinks,
              COALESCE(SUM(CASE WHEN kind = 'file' THEN size ELSE 0 END), 0) AS bytes
            FROM files
            WHERE snapshot_id = ?
            "#,
        )
        .bind(snapshot_id)
        .fetch_one(&mut *connection)
        .await?;

        let block_row = sqlx::query(
            r#"
            SELECT COUNT(*) AS distinct_blocks, COALESCE(SUM(size), 0) AS bytes
            FROM (
              SELECT fc.chunk_hash, MAX(c.size) AS size
              FROM file_chunks fc
              JOIN files f ON f.file_id = fc.file_id
              JOIN chunks c ON c.chunk_hash = fc.chunk_hash
              WHERE f.snapshot_id = ? AND f.kind = 'file'
              GROUP BY fc.chunk_hash
            )
            "#,
        )
        .bind(snapshot_id)
        .fetch_one(&mut *connection)
        .await?;

        let files = FileCounts {
            entries: non_negative_u64(&file_row, "entries"),
            regular_files: non_negative_u64(&file_row, "regular_files"),
            directories: non_negative_u64(&file_row, "directories"),
            symlinks: non_negative_u64(&file_row, "symlinks"),
            bytes: non_negative_u64(&file_row, "bytes"),
        };
        let blocks = BlockCounts {
            distinct: non_negative_u64(&block_row, "distinct_blocks"),
            bytes: non_negative_u64(&block_row, "bytes"),
        };

        let changes = match &context.difference {
            DifferenceContext::FirstSnapshot => {
                prepare_first_snapshot_changes(&mut connection, snapshot_id).await?
            }
            DifferenceContext::BaselineUnavailable => None,
            DifferenceContext::Available {
                snapshot_id: base_snapshot_id,
                ..
            } => Some(
                prepare_baseline_changes(
                    &mut connection,
                    snapshot_id,
                    base_snapshot_id,
                    context.base_table_name(),
                )
                .await?,
            ),
        };
        let block_changes = match changes.as_ref() {
            Some(changes) => {
                Some(prepare_changed_blocks(&mut connection, snapshot_id, changes).await?)
            }
            None => None,
        };
        detach_baseline(&mut connection, attached).await?;

        let change_summary = match (&context.difference, changes.as_ref()) {
            (DifferenceContext::BaselineUnavailable, _) => ChangeSummary {
                state: "baselineUnavailable".to_string(),
                added: 0,
                deleted: 0,
                changed: 0,
            },
            (DifferenceContext::FirstSnapshot, Some(changes)) => ChangeSummary {
                state: "firstSnapshot".to_string(),
                added: changes.counts.added,
                deleted: 0,
                changed: 0,
            },
            (DifferenceContext::Available { .. }, Some(changes)) => ChangeSummary {
                state: "available".to_string(),
                added: changes.counts.added,
                deleted: changes.counts.deleted,
                changed: changes.counts.changed,
            },
            _ => unreachable!("available snapshot differences are prepared"),
        };

        Ok(SnapshotInspectionSession {
            inspector: self.clone(),
            summary: SnapshotSummary {
                snapshot: context.metadata,
                availability: DifferenceAvailability::from_context(&context.difference),
                files,
                changes: change_summary,
                blocks,
            },
            changes,
            block_changes,
        })
    }

    pub async fn files(&self, request: FileInspectionRequest) -> Result<FilePage> {
        request.validate()?;
        let after = decode_file_cursor(&request)?;
        let context = self.resolve_context(&request.snapshot_id).await?;
        if request.scope == FileScope::Changes
            && matches!(context.difference, DifferenceContext::BaselineUnavailable)
        {
            return Err(SnapshotInspectionError::BaselineUnavailable {
                snapshot_id: request.snapshot_id,
            });
        }

        let pool = index_db::open_readonly_index_db(&context.current_path).await?;
        let mut connection = pool.acquire().await?;
        let attached = attach_baseline_if_needed(&mut connection, &context).await?;
        let rows = match (&request.scope, &context.difference) {
            (FileScope::All, _) => {
                fetch_all_files(&mut connection, &request, after.as_deref()).await?
            }
            (FileScope::Changes, DifferenceContext::FirstSnapshot) => {
                fetch_first_snapshot_changes(&mut connection, &request, after.as_deref()).await?
            }
            (FileScope::Changes, DifferenceContext::Available { snapshot_id, .. }) => {
                fetch_baseline_changes(
                    &mut connection,
                    &request,
                    after.as_deref(),
                    snapshot_id,
                    context.base_table_name(),
                )
                .await?
            }
            (FileScope::Changes, DifferenceContext::BaselineUnavailable) => unreachable!(),
        };
        detach_baseline(&mut connection, attached).await?;

        let has_more = rows.len() > request.limit as usize;
        let mut entries = rows;
        if has_more {
            entries.pop();
        }
        let next_cursor = has_more
            .then(|| entries.last().map(|entry| entry.path.clone()))
            .flatten()
            .map(|after| encode_file_cursor(&request, after));
        Ok(FilePage {
            entries,
            next_cursor,
        })
    }

    pub async fn blocks(&self, request: BlockInspectionRequest) -> Result<BlockPage> {
        request.validate()?;
        let session = self.prepare(&request.snapshot_id).await?;
        session.blocks(request).await
    }

    pub async fn storage(&self, request: StorageInspectionRequest) -> Result<StoragePage> {
        request.validate()?;
        let after = decode_storage_cursor(&request)?;
        self.resolve_context(&request.snapshot_id).await?;
        let sidecar_path = self.storage_index_path(&request.snapshot_id);
        if !storage_index_is_complete(&sidecar_path, &request.snapshot_id).await? {
            return Err(SnapshotInspectionError::StorageIndexUnavailable {
                snapshot_id: request.snapshot_id,
            });
        }
        let pool = index_db::open_readonly_index_db(&sidecar_path).await?;
        let query = normalize_query(request.query.as_deref());
        let after = after.as_deref().unwrap_or_default();
        let kind = request.kind.as_deref().unwrap_or_default();
        let mut entries = sqlx::query(
            r#"
            SELECT storage_id, kind, document_bytes, recorded_at, referenced_blocks, logical_bytes
            FROM storage_inspection_objects
            WHERE (? = '' OR kind = ?)
              AND (? = '' OR storage_id LIKE ? || '%')
              AND storage_id > ? COLLATE BINARY
            ORDER BY storage_id COLLATE BINARY
            LIMIT ?
            "#,
        )
        .bind(kind)
        .bind(kind)
        .bind(&query)
        .bind(&query)
        .bind(after)
        .bind(i64::from(request.limit) + 1)
        .fetch_all(&pool)
        .await?
        .into_iter()
        .map(|row| StorageObjectEntry {
            storage_id: row.get("storage_id"),
            kind: row.get("kind"),
            document_bytes: row
                .try_get::<Option<i64>, _>("document_bytes")
                .unwrap_or(None)
                .map(|value| value.max(0) as u64),
            recorded_at: row
                .try_get::<Option<String>, _>("recorded_at")
                .unwrap_or(None),
            referenced_blocks: non_negative_u64(&row, "referenced_blocks"),
            logical_bytes: non_negative_u64(&row, "logical_bytes"),
        })
        .collect::<Vec<_>>();
        let has_more = entries.len() > request.limit as usize;
        if has_more {
            entries.pop();
        }
        let next_cursor = has_more
            .then(|| entries.last().map(|entry| entry.storage_id.clone()))
            .flatten()
            .map(|after| encode_storage_cursor(&request, after));
        Ok(StoragePage {
            entries,
            next_cursor,
        })
    }

    pub async fn storage_blocks(
        &self,
        request: StorageBlocksInspectionRequest,
    ) -> Result<StorageBlocksPage> {
        request.validate()?;
        let after = decode_storage_blocks_cursor(&request)?;
        self.resolve_context(&request.snapshot_id).await?;
        let sidecar_path = self.storage_index_path(&request.snapshot_id);
        if !storage_index_is_complete(&sidecar_path, &request.snapshot_id).await? {
            return Err(SnapshotInspectionError::StorageIndexUnavailable {
                snapshot_id: request.snapshot_id,
            });
        }
        let pool = index_db::open_readonly_index_db(&sidecar_path).await?;
        let Some((after_hash, after_offset, after_length)) = after
            .as_deref()
            .map(parse_storage_block_cursor_key)
            .transpose()?
        else {
            return self
                .storage_blocks_from_index(&pool, &request, "", 0, 0)
                .await;
        };
        self.storage_blocks_from_index(&pool, &request, &after_hash, after_offset, after_length)
            .await
    }

    async fn storage_blocks_from_index(
        &self,
        pool: &sqlx::SqlitePool,
        request: &StorageBlocksInspectionRequest,
        after_hash: &str,
        after_offset: u64,
        after_length: u64,
    ) -> Result<StorageBlocksPage> {
        let rows = sqlx::query(
            r#"
            SELECT b.hash, b.size, b.offset, b.length
            FROM storage_inspection_blocks b
            JOIN storage_inspection_objects o ON o.object_key = b.object_key
            WHERE o.storage_id = ?
              AND (
                   b.hash > ? COLLATE BINARY
                   OR (b.hash = ? AND b.offset > ?)
                   OR (b.hash = ? AND b.offset = ? AND b.length > ?)
              )
            ORDER BY b.hash COLLATE BINARY, b.offset, b.length
            LIMIT ?
            "#,
        )
        .bind(&request.storage_id)
        .bind(after_hash)
        .bind(after_hash)
        .bind(after_offset as i64)
        .bind(after_hash)
        .bind(after_offset as i64)
        .bind(after_length as i64)
        .bind(i64::from(request.limit) + 1)
        .fetch_all(pool)
        .await?;
        if rows.is_empty() && !storage_object_exists(pool, &request.storage_id).await? {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "storage_id is not referenced by the selected snapshot".to_string(),
            });
        }
        let mut entries = rows
            .into_iter()
            .map(|row| StorageBlockEntry {
                hash: row.get("hash"),
                size: non_negative_u64(&row, "size"),
                offset: non_negative_u64(&row, "offset"),
                length: non_negative_u64(&row, "length"),
            })
            .collect::<Vec<_>>();
        let has_more = entries.len() > request.limit as usize;
        if has_more {
            entries.pop();
        }
        let next_cursor = has_more
            .then(|| entries.last().map(StorageBlockEntry::cursor_key))
            .flatten()
            .map(|after| encode_storage_blocks_cursor(request, after));
        Ok(StorageBlocksPage {
            entries,
            next_cursor,
        })
    }

    /// Builds the local, snapshot-scoped Storage sidecar. The final path is never
    /// opened until this complete temporary database has been committed and renamed.
    pub async fn build_storage_index(&self, snapshot_id: &str) -> Result<()> {
        let context = self.resolve_context(snapshot_id).await?;
        let final_path = self.storage_index_path(snapshot_id);
        if storage_index_is_complete(&final_path, snapshot_id).await? {
            return Ok(());
        }
        let parent =
            final_path
                .parent()
                .ok_or_else(|| SnapshotInspectionError::InvalidArgument {
                    message: "storage inspection path has no parent".to_string(),
                })?;
        fs::create_dir_all(parent).map_err(Error::from)?;
        cleanup_stale_storage_index_temps(parent, snapshot_id);
        let temporary_path = parent.join(format!(
            ".{snapshot_id}.{}.storage-index.sqlite",
            uuid::Uuid::new_v4()
        ));
        if temporary_path.exists() {
            fs::remove_file(&temporary_path).map_err(Error::from)?;
        }

        let build = async {
            let sidecar_pool = index_db::open_index_db(&temporary_path).await?;
            ensure_storage_index_schema(&sidecar_pool).await?;

            let storage_pool = index_db::open_readonly_index_db(&self.storage_db_path).await?;
            let metadata_pool = index_db::open_readonly_index_db(&self.storage_db_path).await?;
            let has_storage_metadata = sqlx::query(
                "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'storage_objects'",
            )
            .fetch_optional(&metadata_pool)
            .await?
            .is_some();
            let same_database = self.storage_db_path == context.current_path;
            let mut source_connection = storage_pool.acquire().await?;
            if !same_database {
                sqlx::query("ATTACH DATABASE ? AS snapshot_filemap")
                    .bind(context.current_path.to_string_lossy().into_owned())
                    .execute(&mut *source_connection)
                    .await?;
            }
            let source_sql = if same_database {
                r#"
                WITH snapshot_chunks AS (
                    SELECT fc.chunk_hash AS hash, MAX(c.size) AS size, MAX(fc.len) AS len
                    FROM file_chunks fc
                    JOIN files f ON f.file_id = fc.file_id
                    JOIN chunks c ON c.chunk_hash = fc.chunk_hash
                    WHERE f.snapshot_id = ? AND f.kind = 'file'
                    GROUP BY fc.chunk_hash
                )
                SELECT sc.hash, sc.size, sc.len, co.provider, co.object_id AS encoded
                FROM snapshot_chunks sc
                JOIN chunk_objects co ON co.chunk_hash = sc.hash
                ORDER BY sc.hash COLLATE BINARY
                "#
            } else {
                r#"
                WITH snapshot_chunks AS (
                    SELECT fc.chunk_hash AS hash, MAX(c.size) AS size, MAX(fc.len) AS len
                    FROM snapshot_filemap.file_chunks fc
                    JOIN snapshot_filemap.files f ON f.file_id = fc.file_id
                    JOIN snapshot_filemap.chunks c ON c.chunk_hash = fc.chunk_hash
                    WHERE f.snapshot_id = ? AND f.kind = 'file'
                    GROUP BY fc.chunk_hash
                )
                SELECT sc.hash, sc.size, sc.len, co.provider, co.object_id AS encoded
                FROM snapshot_chunks sc
                JOIN chunk_objects co ON co.chunk_hash = sc.hash
                ORDER BY sc.hash COLLATE BINARY
                "#
            };
            let mut rows = sqlx::query(source_sql)
                .bind(snapshot_id)
                .fetch(&mut *source_connection);
            let mut pending = Vec::with_capacity(STORAGE_INDEX_BUILD_BATCH_SIZE);
            let mut metadata_cache = HashMap::new();
            while let Some(row) = rows.try_next().await? {
                pending.push(StorageChunkRow {
                    hash: row.get("hash"),
                    size: non_negative_u64(&row, "size"),
                    len: non_negative_u64(&row, "len"),
                    provider: row.get("provider"),
                    encoded: row.get("encoded"),
                });
                if pending.len() >= STORAGE_INDEX_BUILD_BATCH_SIZE {
                    write_storage_index_batch(
                        &sidecar_pool,
                        &metadata_pool,
                        has_storage_metadata,
                        &mut metadata_cache,
                        &mut pending,
                    )
                    .await?;
                }
            }
            write_storage_index_batch(
                &sidecar_pool,
                &metadata_pool,
                has_storage_metadata,
                &mut metadata_cache,
                &mut pending,
            )
            .await?;
            drop(rows);
            if !same_database {
                sqlx::query("DETACH DATABASE snapshot_filemap")
                    .execute(&mut *source_connection)
                    .await?;
            }
            sqlx::query(
                "INSERT INTO storage_inspection_meta (schema_version, snapshot_id) VALUES (?, ?)",
            )
            .bind(STORAGE_INDEX_SCHEMA_VERSION)
            .bind(snapshot_id)
            .execute(&sidecar_pool)
            .await?;
            drop(source_connection);
            drop(metadata_pool);
            drop(storage_pool);
            drop(sidecar_pool);
            Ok::<(), SnapshotInspectionError>(())
        }
        .await;

        match build {
            Ok(()) => {
                fs::rename(&temporary_path, &final_path).map_err(Error::from)?;
                Ok(())
            }
            Err(error) => {
                let _ = fs::remove_file(&temporary_path);
                Err(error)
            }
        }
    }

    async fn blocks_page(&self, request: BlockInspectionRequest) -> Result<BlockPage> {
        request.validate()?;
        let after = decode_block_cursor(&request)?;
        let context = self.resolve_context(&request.snapshot_id).await?;
        let pool = index_db::open_readonly_index_db(&context.current_path).await?;
        let query_text = normalize_query(request.query.as_deref());
        let rows = sqlx::query(
            r#"
            SELECT fc.chunk_hash AS hash, MAX(c.size) AS size, COUNT(DISTINCT f.file_id) AS referencing_files
            FROM file_chunks fc
            JOIN files f ON f.file_id = fc.file_id
            JOIN chunks c ON c.chunk_hash = fc.chunk_hash
            WHERE f.snapshot_id = ?
              AND f.kind = 'file'
              AND (? = '' OR fc.chunk_hash LIKE ? || '%')
              AND (? = '' OR fc.chunk_hash > ? COLLATE BINARY)
            GROUP BY fc.chunk_hash
            ORDER BY fc.chunk_hash COLLATE BINARY
            LIMIT ?
            "#,
        )
        .bind(&request.snapshot_id)
        .bind(&query_text)
        .bind(&query_text)
        .bind(after.as_deref().unwrap_or_default())
        .bind(after.as_deref().unwrap_or_default())
        .bind(i64::from(request.limit) + 1)
        .fetch_all(&pool)
        .await?;
        let mut entries = rows
            .into_iter()
            .map(|row| BlockEntry {
                hash: row.get("hash"),
                size: non_negative_u64(&row, "size"),
                changed_files: 0,
                referencing_files: non_negative_u64(&row, "referencing_files"),
            })
            .collect::<Vec<_>>();
        let has_more = entries.len() > request.limit as usize;
        if has_more {
            entries.pop();
        }
        let next_cursor = has_more
            .then(|| entries.last().map(|entry| entry.hash.clone()))
            .flatten()
            .map(|after| encode_block_cursor(&request, after));
        Ok(BlockPage {
            entries,
            next_cursor,
        })
    }

    async fn resolve_context(&self, snapshot_id: &str) -> Result<InspectionContext> {
        if snapshot_id.trim().is_empty() {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "snapshot_id must not be empty".to_string(),
            });
        }
        let endpoint_pool = index_db::open_readonly_index_db(&self.endpoint_db_path).await?;
        let row = sqlx::query(
            "SELECT snapshot_id, created_at, source_path, label, base_snapshot_id FROM snapshots WHERE snapshot_id = ?",
        )
        .bind(snapshot_id)
        .fetch_optional(&endpoint_pool)
        .await?;
        let Some(row) = row else {
            return Err(SnapshotInspectionError::SnapshotNotFound {
                snapshot_id: snapshot_id.to_string(),
            });
        };
        let metadata = SnapshotMetadata {
            snapshot_id: row.get("snapshot_id"),
            created_at: row.get("created_at"),
            source_path: row.get("source_path"),
            label: row.get("label"),
            base_snapshot_id: row.get("base_snapshot_id"),
        };
        let current_path = self.data_path_for(&endpoint_pool, snapshot_id).await?;
        let difference = match metadata.base_snapshot_id.as_deref() {
            None => DifferenceContext::FirstSnapshot,
            Some(base_snapshot_id) => {
                let exists = sqlx::query("SELECT 1 FROM snapshots WHERE snapshot_id = ? LIMIT 1")
                    .bind(base_snapshot_id)
                    .fetch_optional(&endpoint_pool)
                    .await?
                    .is_some();
                if !exists {
                    DifferenceContext::BaselineUnavailable
                } else {
                    match self.data_path_for(&endpoint_pool, base_snapshot_id).await {
                        Ok(path) => DifferenceContext::Available {
                            snapshot_id: base_snapshot_id.to_string(),
                            path,
                        },
                        Err(SnapshotInspectionError::FilemapUnavailable { .. })
                        | Err(SnapshotInspectionError::SnapshotNotRetained { .. }) => {
                            DifferenceContext::BaselineUnavailable
                        }
                        Err(error) => return Err(error),
                    }
                }
            }
        };

        Ok(InspectionContext {
            metadata,
            current_path,
            difference,
        })
    }

    async fn data_path_for(
        &self,
        endpoint_pool: &sqlx::SqlitePool,
        snapshot_id: &str,
    ) -> Result<PathBuf> {
        let filemap_path = self.filemap_path(snapshot_id);
        if filemap_path.is_file() {
            return Ok(filemap_path);
        }
        // The pre-two-level layout used a single global `index.sqlite` as both the
        // endpoint index and filemap. It remains readable even when a valid
        // retained snapshot contains no file rows.
        if self
            .endpoint_db_path
            .file_name()
            .and_then(|name| name.to_str())
            == Some("index.sqlite")
        {
            return Ok(self.endpoint_db_path.clone());
        }
        let legacy_filemap = sqlx::query("SELECT 1 FROM files WHERE snapshot_id = ? LIMIT 1")
            .bind(snapshot_id)
            .fetch_optional(endpoint_pool)
            .await?
            .is_some();
        if legacy_filemap {
            return Ok(self.endpoint_db_path.clone());
        }
        let retained = sqlx::query("SELECT 1 FROM snapshots WHERE snapshot_id = ? LIMIT 1")
            .bind(snapshot_id)
            .fetch_optional(endpoint_pool)
            .await?
            .is_some();
        if !retained {
            return Err(SnapshotInspectionError::SnapshotNotRetained {
                snapshot_id: snapshot_id.to_string(),
            });
        }
        Err(SnapshotInspectionError::FilemapUnavailable {
            snapshot_id: snapshot_id.to_string(),
            message: "the retained filemap is not cached locally".to_string(),
        })
    }
}

/// A precomputed direct-baseline comparison for a single retained snapshot.
///
/// This intentionally lives in memory. It is derived from retained filemaps
/// and is discarded when its owning daemon exits, so it does not create a new
/// retained file-history surface.
#[derive(Clone)]
pub struct SnapshotInspectionSession {
    inspector: SnapshotInspector,
    summary: SnapshotSummary,
    changes: Option<PreparedChanges>,
    block_changes: Option<PreparedBlockChanges>,
}

impl SnapshotInspectionSession {
    pub fn summary(&self) -> SnapshotSummary {
        self.summary.clone()
    }

    pub async fn files(&self, request: FileInspectionRequest) -> Result<FilePage> {
        request.validate()?;
        if request.scope == FileScope::All {
            return self.inspector.files(request).await;
        }
        let Some(changes) = &self.changes else {
            return Err(SnapshotInspectionError::BaselineUnavailable {
                snapshot_id: request.snapshot_id,
            });
        };
        changes.files(&request)
    }

    pub async fn blocks(&self, request: BlockInspectionRequest) -> Result<BlockPage> {
        request.validate()?;
        if request.changes_only {
            let Some(block_changes) = &self.block_changes else {
                return Err(SnapshotInspectionError::BaselineUnavailable {
                    snapshot_id: request.snapshot_id,
                });
            };
            return block_changes.page(&request);
        }

        let mut query = request;
        query.changes_only = false;
        let mut page = self.inspector.blocks_page(query).await?;
        if let Some(block_changes) = &self.block_changes {
            for entry in &mut page.entries {
                entry.changed_files = block_changes
                    .entries
                    .get(&entry.hash)
                    .map(|block| block.changed_files)
                    .unwrap_or(0);
            }
        }
        Ok(page)
    }

    pub async fn storage(&self, request: StorageInspectionRequest) -> Result<StoragePage> {
        request.validate()?;
        self.inspector.storage(request).await
    }

    pub async fn storage_blocks(
        &self,
        request: StorageBlocksInspectionRequest,
    ) -> Result<StorageBlocksPage> {
        request.validate()?;
        self.inspector.storage_blocks(request).await
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMetadata {
    pub snapshot_id: String,
    pub created_at: String,
    pub source_path: String,
    pub label: String,
    pub base_snapshot_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSummary {
    pub snapshot: SnapshotMetadata,
    pub availability: DifferenceAvailability,
    pub files: FileCounts,
    pub changes: ChangeSummary,
    pub blocks: BlockCounts,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DifferenceAvailability {
    pub state: String,
    pub reason: Option<String>,
}

impl DifferenceAvailability {
    fn from_context(context: &DifferenceContext) -> Self {
        match context {
            DifferenceContext::Available { .. } => Self {
                state: "available".to_string(),
                reason: None,
            },
            DifferenceContext::FirstSnapshot => Self {
                state: "firstSnapshot".to_string(),
                reason: None,
            },
            DifferenceContext::BaselineUnavailable => Self {
                state: "baselineUnavailable".to_string(),
                reason: Some(
                    "The direct baseline is no longer retained or locally available.".to_string(),
                ),
            },
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCounts {
    pub entries: u64,
    pub regular_files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub state: String,
    pub added: u64,
    pub deleted: u64,
    pub changed: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockCounts {
    pub distinct: u64,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilePresentation {
    Tree,
    List,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileScope {
    All,
    Changes,
}

#[derive(Debug, Clone)]
pub struct FileInspectionRequest {
    pub snapshot_id: String,
    pub presentation: FilePresentation,
    pub scope: FileScope,
    pub parent: Option<String>,
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: u16,
}

impl FileInspectionRequest {
    fn validate(&self) -> Result<()> {
        validate_page_size(self.limit)?;
        if self.snapshot_id.trim().is_empty() {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "snapshot_id must not be empty".to_string(),
            });
        }
        validate_relative_path(self.parent.as_deref(), "parent")?;
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct BlockInspectionRequest {
    pub snapshot_id: String,
    pub changes_only: bool,
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: u16,
}

impl BlockInspectionRequest {
    fn validate(&self) -> Result<()> {
        validate_page_size(self.limit)?;
        if self.snapshot_id.trim().is_empty() {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "snapshot_id must not be empty".to_string(),
            });
        }
        if self
            .query
            .as_deref()
            .is_some_and(|query| !query.bytes().all(|byte| byte.is_ascii_hexdigit()))
        {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "block query must be a hexadecimal hash prefix".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePage {
    pub entries: Vec<FileEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub change: String,
    pub is_ancestor_context: bool,
    pub size: u64,
    pub mtime_ms: i64,
    pub mode: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub baseline: Option<FileMetadata>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub descendant_changes: Option<ChangeCounts>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    pub kind: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub mode: i64,
}

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeCounts {
    pub added: u64,
    pub deleted: u64,
    pub changed: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockPage {
    pub entries: Vec<BlockEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BlockEntry {
    pub hash: String,
    pub size: u64,
    pub changed_files: u64,
    pub referencing_files: u64,
}

#[derive(Debug, Clone)]
pub struct StorageInspectionRequest {
    pub snapshot_id: String,
    pub kind: Option<String>,
    pub query: Option<String>,
    pub cursor: Option<String>,
    pub limit: u16,
}

impl StorageInspectionRequest {
    fn validate(&self) -> Result<()> {
        validate_page_size(self.limit)?;
        if self.snapshot_id.trim().is_empty() {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "snapshot_id must not be empty".to_string(),
            });
        }
        if self
            .kind
            .as_deref()
            .is_some_and(|kind| kind != "pack" && kind != "direct")
        {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "storage kind must be pack or direct".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct StorageBlocksInspectionRequest {
    pub snapshot_id: String,
    pub storage_id: String,
    pub cursor: Option<String>,
    pub limit: u16,
}

impl StorageBlocksInspectionRequest {
    fn validate(&self) -> Result<()> {
        validate_page_size(self.limit)?;
        if self.snapshot_id.trim().is_empty() || self.storage_id.trim().is_empty() {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "snapshot_id and storage_id must not be empty".to_string(),
            });
        }
        if !self.storage_id.starts_with("sto_") {
            return Err(SnapshotInspectionError::InvalidArgument {
                message: "storage_id must be an opaque storage identifier".to_string(),
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoragePage {
    pub entries: Vec<StorageObjectEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageObjectEntry {
    pub storage_id: String,
    pub kind: String,
    pub document_bytes: Option<u64>,
    pub recorded_at: Option<String>,
    pub referenced_blocks: u64,
    pub logical_bytes: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageBlocksPage {
    pub entries: Vec<StorageBlockEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StorageBlockEntry {
    pub hash: String,
    pub size: u64,
    pub offset: u64,
    pub length: u64,
}

impl StorageBlockEntry {
    fn cursor_key(&self) -> String {
        format!("{}:{:020}:{:020}", self.hash, self.offset, self.length)
    }
}

#[derive(Debug, Clone)]
struct StorageChunkRow {
    hash: String,
    size: u64,
    len: u64,
    provider: String,
    encoded: String,
}

#[derive(Clone)]
struct StorageObjectMetadata {
    document_bytes: Option<i64>,
    recorded_at: Option<String>,
}

fn default_storage_index_dir(filemap_dir: &Path) -> PathBuf {
    let endpoint_id = filemap_dir.file_name().unwrap_or_default();
    let direct_parent = filemap_dir.parent().unwrap_or(filemap_dir);
    let index_root = (direct_parent.file_name().and_then(|name| name.to_str()) == Some("filemaps"))
        .then(|| direct_parent.parent())
        .flatten()
        .unwrap_or(direct_parent);
    index_root.join("storage-inspection").join(endpoint_id)
}

fn cleanup_stale_storage_index_temps(parent: &Path, snapshot_id: &str) {
    let prefix = format!(".{snapshot_id}.");
    let suffix = ".storage-index.sqlite";
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(suffix) {
            continue;
        }
        let is_stale = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| SystemTime::now().duration_since(modified).ok())
            .is_some_and(|age| age >= STORAGE_INDEX_TEMP_MAX_AGE);
        if is_stale {
            let _ = fs::remove_file(path);
        }
    }
}

async fn ensure_storage_index_schema(pool: &sqlx::SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS storage_inspection_meta (
            schema_version INTEGER NOT NULL,
            snapshot_id TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS storage_inspection_objects (
            object_key INTEGER PRIMARY KEY,
            storage_id TEXT NOT NULL UNIQUE,
            kind TEXT NOT NULL CHECK (kind IN ('pack', 'direct')),
            document_bytes INTEGER NULL,
            recorded_at TEXT NULL,
            referenced_blocks INTEGER NOT NULL,
            logical_bytes INTEGER NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS storage_inspection_blocks (
            object_key INTEGER NOT NULL REFERENCES storage_inspection_objects(object_key),
            hash TEXT NOT NULL,
            size INTEGER NOT NULL,
            offset INTEGER NOT NULL,
            length INTEGER NOT NULL,
            PRIMARY KEY (object_key, hash, offset, length)
        )
        "#,
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_storage_inspection_objects_page ON storage_inspection_objects(kind, storage_id)",
    )
    .execute(pool)
    .await?;
    sqlx::query(
        "CREATE INDEX IF NOT EXISTS idx_storage_inspection_blocks_page ON storage_inspection_blocks(object_key, hash, offset, length)",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn storage_index_is_complete(path: &Path, snapshot_id: &str) -> Result<bool> {
    if !path.is_file() {
        return Ok(false);
    }
    let pool = match index_db::open_readonly_index_db(path).await {
        Ok(pool) => pool,
        Err(_) => return Ok(false),
    };
    let row = match sqlx::query(
        "SELECT 1 FROM storage_inspection_meta WHERE schema_version = ? AND snapshot_id = ? LIMIT 1",
    )
    .bind(STORAGE_INDEX_SCHEMA_VERSION)
    .bind(snapshot_id)
    .fetch_optional(&pool)
    .await
    {
        Ok(row) => row,
        Err(_) => return Ok(false),
    };
    Ok(row.is_some())
}

async fn storage_object_exists(pool: &sqlx::SqlitePool, storage_id: &str) -> Result<bool> {
    Ok(
        sqlx::query("SELECT 1 FROM storage_inspection_objects WHERE storage_id = ? LIMIT 1")
            .bind(storage_id)
            .fetch_optional(pool)
            .await?
            .is_some(),
    )
}

fn parse_storage_block_cursor_key(value: &str) -> Result<(String, u64, u64)> {
    let mut parts = value.split(':');
    let hash = parts.next().unwrap_or_default();
    let offset = parts.next();
    let length = parts.next();
    if hash.is_empty() || parts.next().is_some() {
        return Err(SnapshotInspectionError::InvalidCursor {
            message: "storage block cursor payload is invalid".to_string(),
        });
    }
    let offset = offset
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| SnapshotInspectionError::InvalidCursor {
            message: "storage block cursor payload is invalid".to_string(),
        })?;
    let length = length
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| SnapshotInspectionError::InvalidCursor {
            message: "storage block cursor payload is invalid".to_string(),
        })?;
    Ok((hash.to_string(), offset, length))
}

async fn write_storage_index_batch(
    sidecar_pool: &sqlx::SqlitePool,
    metadata_pool: &sqlx::SqlitePool,
    has_storage_metadata: bool,
    metadata_cache: &mut HashMap<(String, String), StorageObjectMetadata>,
    rows: &mut Vec<StorageChunkRow>,
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut transaction = sidecar_pool.begin().await?;
    for row in rows.drain(..) {
        let (kind, object_id, offset, length) = match parse_chunk_object_ref(&row.encoded)? {
            ChunkObjectRef::Direct { object_id } => ("direct", object_id, 0, row.len),
            ChunkObjectRef::PackSlice {
                pack_object_id,
                offset,
                len,
            } => ("pack", pack_object_id, offset, len),
        };
        let metadata_key = (row.provider.clone(), object_id.clone());
        let metadata = if let Some(metadata) = metadata_cache.get(&metadata_key) {
            metadata.clone()
        } else {
            let metadata = if has_storage_metadata {
                sqlx::query(
                    "SELECT document_bytes, recorded_at FROM storage_objects WHERE provider = ? AND object_id = ? LIMIT 1",
                )
                .bind(&row.provider)
                .bind(&object_id)
                .fetch_optional(metadata_pool)
                .await?
                .map(|metadata| StorageObjectMetadata {
                    document_bytes: metadata
                        .try_get::<Option<i64>, _>("document_bytes")
                        .unwrap_or(None),
                    recorded_at: metadata
                        .try_get::<Option<String>, _>("recorded_at")
                        .unwrap_or(None),
                })
                .unwrap_or(StorageObjectMetadata {
                    document_bytes: None,
                    recorded_at: None,
                })
            } else {
                StorageObjectMetadata {
                    document_bytes: None,
                    recorded_at: None,
                }
            };
            metadata_cache.insert(metadata_key, metadata.clone());
            metadata
        };
        let storage_id = storage_object_id(&row.provider, &object_id);
        let logical_bytes =
            i64::try_from(row.size).map_err(|_| SnapshotInspectionError::InvalidArgument {
                message: "logical block size exceeds SQLite range".to_string(),
            })?;
        sqlx::query(
            r#"
            INSERT INTO storage_inspection_objects
              (storage_id, kind, document_bytes, recorded_at, referenced_blocks, logical_bytes)
            VALUES (?, ?, ?, ?, 1, ?)
            ON CONFLICT(storage_id) DO UPDATE SET
              referenced_blocks = referenced_blocks + 1,
              logical_bytes = logical_bytes + excluded.logical_bytes
            "#,
        )
        .bind(&storage_id)
        .bind(kind)
        .bind(metadata.document_bytes)
        .bind(metadata.recorded_at)
        .bind(logical_bytes)
        .execute(&mut *transaction)
        .await?;
        let object_key: i64 = sqlx::query_scalar(
            "SELECT object_key FROM storage_inspection_objects WHERE storage_id = ?",
        )
        .bind(&storage_id)
        .fetch_one(&mut *transaction)
        .await?;
        sqlx::query(
            r#"
            INSERT OR IGNORE INTO storage_inspection_blocks (object_key, hash, size, offset, length)
            VALUES (?, ?, ?, ?, ?)
            "#,
        )
        .bind(object_key)
        .bind(&row.hash)
        .bind(logical_bytes)
        .bind(
            i64::try_from(offset).map_err(|_| SnapshotInspectionError::InvalidArgument {
                message: "storage slice offset exceeds SQLite range".to_string(),
            })?,
        )
        .bind(
            i64::try_from(length).map_err(|_| SnapshotInspectionError::InvalidArgument {
                message: "storage slice length exceeds SQLite range".to_string(),
            })?,
        )
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

#[derive(Clone)]
struct PreparedChanges {
    entries: HashMap<String, PreparedFile>,
    tree_paths: HashMap<String, Vec<String>>,
    list_paths: Vec<String>,
    counts: ChangeCounts,
}

#[derive(Clone, Default)]
struct PreparedBlockChanges {
    entries: HashMap<String, PreparedBlock>,
    list_paths: Vec<String>,
}

#[derive(Clone)]
struct PreparedBlock {
    size: u64,
    changed_files: u64,
    referencing_files: u64,
}

#[derive(Clone)]
struct PreparedFile {
    path: String,
    kind: String,
    change: String,
    size: u64,
    mtime_ms: i64,
    mode: i64,
    baseline: Option<FileMetadata>,
    descendant_changes: ChangeCounts,
}

impl PreparedFile {
    fn to_file_entry(&self, tree: bool) -> FileEntry {
        FileEntry {
            path: self.path.clone(),
            name: self
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&self.path)
                .to_string(),
            kind: self.kind.clone(),
            change: self.change.clone(),
            is_ancestor_context: tree && self.change == "unchanged",
            size: self.size,
            mtime_ms: self.mtime_ms,
            mode: self.mode,
            baseline: self.baseline.clone(),
            descendant_changes: (tree && self.kind == "dir")
                .then(|| self.descendant_changes.clone()),
        }
    }
}

impl PreparedChanges {
    fn files(&self, request: &FileInspectionRequest) -> Result<FilePage> {
        let after = decode_file_cursor(request)?;
        let tree = request.presentation == FilePresentation::Tree;
        let paths = if tree {
            self.tree_paths
                .get(request.parent.as_deref().unwrap_or_default())
                .map(Vec::as_slice)
                .unwrap_or_default()
        } else {
            self.list_paths.as_slice()
        };
        let query = normalize_query(request.query.as_deref()).to_ascii_lowercase();
        let after = after.as_deref().unwrap_or_default();
        let mut entries = paths
            .iter()
            .filter(|path| path.as_bytes() > after.as_bytes())
            .filter(|path| query.is_empty() || path.to_ascii_lowercase().contains(&query))
            .filter_map(|path| self.entries.get(path))
            .take(request.limit as usize + 1)
            .map(|entry| entry.to_file_entry(tree))
            .collect::<Vec<_>>();
        let has_more = entries.len() > request.limit as usize;
        if has_more {
            entries.pop();
        }
        let next_cursor = has_more
            .then(|| entries.last().map(|entry| entry.path.clone()))
            .flatten()
            .map(|after| encode_file_cursor(request, after));
        Ok(FilePage {
            entries,
            next_cursor,
        })
    }
}

impl PreparedBlockChanges {
    fn page(&self, request: &BlockInspectionRequest) -> Result<BlockPage> {
        let after = decode_block_cursor(request)?;
        let query = normalize_query(request.query.as_deref()).to_ascii_lowercase();
        let after = after.as_deref().unwrap_or_default();
        let mut entries = self
            .list_paths
            .iter()
            .filter(|path| path.as_bytes() > after.as_bytes())
            .filter(|path| query.is_empty() || path.starts_with(&query))
            .filter_map(|path| self.entries.get(path).map(|block| (path, block)))
            .take(request.limit as usize + 1)
            .map(|(hash, block)| BlockEntry {
                hash: hash.clone(),
                size: block.size,
                changed_files: block.changed_files,
                referencing_files: block.referencing_files,
            })
            .collect::<Vec<_>>();
        let has_more = entries.len() > request.limit as usize;
        if has_more {
            entries.pop();
        }
        let next_cursor = has_more
            .then(|| entries.last().map(|entry| entry.hash.clone()))
            .flatten()
            .map(|after| encode_block_cursor(request, after));
        Ok(BlockPage {
            entries,
            next_cursor,
        })
    }
}

async fn prepare_first_snapshot_changes(
    connection: &mut SqliteConnection,
    snapshot_id: &str,
) -> Result<Option<PreparedChanges>> {
    let rows = sqlx::query(
        r#"
        SELECT path, kind, size, mtime_ms, mode
        FROM files
        WHERE snapshot_id = ?
        ORDER BY path COLLATE BINARY
        "#,
    )
    .bind(snapshot_id)
    .fetch_all(&mut *connection)
    .await?;
    let files = rows
        .into_iter()
        .map(|row| PreparedFile {
            path: row.get("path"),
            kind: row.get("kind"),
            change: "added".to_string(),
            size: non_negative_u64(&row, "size"),
            mtime_ms: row.get("mtime_ms"),
            mode: row.get("mode"),
            baseline: None,
            descendant_changes: ChangeCounts::default(),
        })
        .collect();
    Ok(Some(prepare_changes_index(files)))
}

async fn prepare_baseline_changes(
    connection: &mut SqliteConnection,
    snapshot_id: &str,
    base_snapshot_id: &str,
    base_table: &str,
) -> Result<PreparedChanges> {
    let cte = changes_cte(base_table);
    let sql = format!(
        r#"
        {cte}
        SELECT path, kind, size, mtime_ms, mode,
               baseline_kind, baseline_size, baseline_mtime_ms, baseline_mode, change
        FROM changes
        WHERE change != 'unchanged'
        ORDER BY path COLLATE BINARY
        "#
    );
    let rows = bind_changes_query(
        sqlx::query(&sql),
        base_snapshot_id,
        snapshot_id,
        snapshot_id,
        base_snapshot_id,
    )
    .fetch_all(&mut *connection)
    .await?;
    let mut files = rows
        .into_iter()
        .map(|row| {
            let change: String = row.get("change");
            let baseline = match change.as_str() {
                "added" | "unchanged" => None,
                _ => Some(FileMetadata {
                    kind: row.get("baseline_kind"),
                    size: non_negative_u64(&row, "baseline_size"),
                    mtime_ms: row.get("baseline_mtime_ms"),
                    mode: row.get("baseline_mode"),
                }),
            };
            PreparedFile {
                path: row.get("path"),
                kind: row.get("kind"),
                change,
                size: non_negative_u64(&row, "size"),
                mtime_ms: row.get("mtime_ms"),
                mode: row.get("mode"),
                baseline,
                descendant_changes: ChangeCounts::default(),
            }
        })
        .collect::<Vec<_>>();

    // The UI needs unchanged directories only as tree context. Fetching them by
    // exact path avoids holding every unchanged file in a large snapshot in the
    // daemon just to prepare one changes-only tree.
    let changed_paths = files
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<HashSet<_>>();
    let mut ancestor_paths = HashSet::new();
    for entry in &files {
        let mut parent = parent_path(&entry.path);
        while !parent.is_empty() {
            if !changed_paths.contains(parent) {
                ancestor_paths.insert(parent.to_string());
            }
            parent = parent_path(parent);
        }
    }
    let mut ancestor_paths = ancestor_paths.into_iter().collect::<Vec<_>>();
    ancestor_paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    for paths in ancestor_paths.chunks(900) {
        let placeholders = vec!["?"; paths.len()].join(", ");
        let sql = format!(
            "SELECT path, kind, size, mtime_ms, mode FROM files \
             WHERE snapshot_id = ? AND kind = 'dir' AND path IN ({placeholders})"
        );
        let mut query = sqlx::query(&sql).bind(snapshot_id);
        for path in paths {
            query = query.bind(path);
        }
        files.extend(
            query
                .fetch_all(&mut *connection)
                .await?
                .into_iter()
                .map(|row| PreparedFile {
                    path: row.get("path"),
                    kind: row.get("kind"),
                    change: "unchanged".to_string(),
                    size: non_negative_u64(&row, "size"),
                    mtime_ms: row.get("mtime_ms"),
                    mode: row.get("mode"),
                    baseline: None,
                    descendant_changes: ChangeCounts::default(),
                }),
        );
    }
    Ok(prepare_changes_index(files))
}

async fn prepare_changed_blocks(
    connection: &mut SqliteConnection,
    snapshot_id: &str,
    changes: &PreparedChanges,
) -> Result<PreparedBlockChanges> {
    let mut changed_paths = changes
        .entries
        .values()
        .filter(|entry| entry.change != "unchanged" && entry.kind == "file")
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    changed_paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));

    let mut entries = HashMap::<String, PreparedBlock>::new();
    for paths in changed_paths.chunks(900) {
        let placeholders = vec!["?"; paths.len()].join(", ");
        let sql = format!(
            "SELECT fc.chunk_hash AS hash, MAX(c.size) AS size, \
             COUNT(DISTINCT f.file_id) AS changed_files \
             FROM file_chunks fc \
             JOIN files f ON f.file_id = fc.file_id \
             JOIN chunks c ON c.chunk_hash = fc.chunk_hash \
             WHERE f.snapshot_id = ? AND f.kind = 'file' AND f.path IN ({placeholders}) \
             GROUP BY fc.chunk_hash"
        );
        let mut query = sqlx::query(&sql).bind(snapshot_id);
        for path in paths {
            query = query.bind(path);
        }
        for row in query.fetch_all(&mut *connection).await? {
            let hash: String = row.get("hash");
            let entry = entries.entry(hash).or_insert_with(|| PreparedBlock {
                size: 0,
                changed_files: 0,
                referencing_files: 0,
            });
            entry.size = entry.size.max(non_negative_u64(&row, "size"));
            entry.changed_files += non_negative_u64(&row, "changed_files");
        }
    }

    let hashes = entries.keys().cloned().collect::<Vec<_>>();
    for hashes in hashes.chunks(900) {
        let placeholders = vec!["?"; hashes.len()].join(", ");
        let sql = format!(
            "SELECT fc.chunk_hash AS hash, COUNT(DISTINCT f.file_id) AS referencing_files \
             FROM file_chunks fc \
             JOIN files f ON f.file_id = fc.file_id \
             WHERE f.snapshot_id = ? AND f.kind = 'file' AND fc.chunk_hash IN ({placeholders}) \
             GROUP BY fc.chunk_hash"
        );
        let mut query = sqlx::query(&sql).bind(snapshot_id);
        for hash in hashes {
            query = query.bind(hash);
        }
        for row in query.fetch_all(&mut *connection).await? {
            let hash: String = row.get("hash");
            if let Some(entry) = entries.get_mut(&hash) {
                entry.referencing_files = non_negative_u64(&row, "referencing_files");
            }
        }
    }

    let mut list_paths = entries.keys().cloned().collect::<Vec<_>>();
    list_paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    Ok(PreparedBlockChanges {
        entries,
        list_paths,
    })
}

fn prepare_changes_index(files: Vec<PreparedFile>) -> PreparedChanges {
    let changed_paths = files
        .iter()
        .filter(|entry| entry.change != "unchanged")
        .map(|entry| entry.path.clone())
        .collect::<HashSet<_>>();
    let mut required_paths = changed_paths.clone();
    for path in &changed_paths {
        let mut parent = parent_path(path);
        while !parent.is_empty() {
            required_paths.insert(parent.to_string());
            parent = parent_path(parent);
        }
    }

    let mut entries = files
        .into_iter()
        .filter(|entry| {
            entry.change != "unchanged"
                || (entry.kind == "dir" && required_paths.contains(&entry.path))
        })
        .map(|entry| (entry.path.clone(), entry))
        .collect::<HashMap<_, _>>();

    let mut counts = ChangeCounts::default();
    let changed_entries = entries
        .values()
        .filter(|entry| entry.change != "unchanged")
        .cloned()
        .collect::<Vec<_>>();
    for entry in &changed_entries {
        match entry.change.as_str() {
            "added" => counts.added += 1,
            "deleted" => counts.deleted += 1,
            "changed" => counts.changed += 1,
            _ => {}
        }
        let mut parent = parent_path(&entry.path);
        while !parent.is_empty() {
            if let Some(ancestor) = entries.get_mut(parent) {
                match entry.change.as_str() {
                    "added" => ancestor.descendant_changes.added += 1,
                    "deleted" => ancestor.descendant_changes.deleted += 1,
                    "changed" => ancestor.descendant_changes.changed += 1,
                    _ => {}
                }
            }
            parent = parent_path(parent);
        }
    }

    let mut tree_paths = HashMap::<String, Vec<String>>::new();
    for entry in entries.values() {
        tree_paths
            .entry(parent_path(&entry.path).to_string())
            .or_default()
            .push(entry.path.clone());
    }
    for paths in tree_paths.values_mut() {
        paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
    }
    let mut list_paths = changed_paths.into_iter().collect::<Vec<_>>();
    list_paths.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));

    // Keep only the paths whose metadata can be presented through the changes views.
    entries.retain(|path, entry| entry.change != "unchanged" || required_paths.contains(path));
    PreparedChanges {
        entries,
        tree_paths,
        list_paths,
        counts,
    }
}

fn parent_path(path: &str) -> &str {
    path.rsplit_once('/')
        .map(|(parent, _)| parent)
        .unwrap_or("")
}

struct InspectionContext {
    metadata: SnapshotMetadata,
    current_path: PathBuf,
    difference: DifferenceContext,
}

impl InspectionContext {
    fn base_table_name(&self) -> &'static str {
        match &self.difference {
            DifferenceContext::Available { path, .. } if path == &self.current_path => "files",
            DifferenceContext::Available { .. } => "base.files",
            DifferenceContext::FirstSnapshot | DifferenceContext::BaselineUnavailable => "files",
        }
    }
}

enum DifferenceContext {
    Available { snapshot_id: String, path: PathBuf },
    FirstSnapshot,
    BaselineUnavailable,
}

async fn attach_baseline_if_needed(
    connection: &mut SqliteConnection,
    context: &InspectionContext,
) -> Result<bool> {
    let DifferenceContext::Available { path, .. } = &context.difference else {
        return Ok(false);
    };
    if path == &context.current_path {
        return Ok(false);
    }
    let path_sql = path.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("ATTACH DATABASE '{path_sql}' AS base"))
        .execute(&mut *connection)
        .await?;
    Ok(true)
}

async fn detach_baseline(connection: &mut SqliteConnection, attached: bool) -> Result<()> {
    if attached {
        sqlx::query("DETACH DATABASE base")
            .execute(&mut *connection)
            .await?;
    }
    Ok(())
}

fn bind_changes_query<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    base_snapshot_id: &'q str,
    current_snapshot_id: &'q str,
    current_snapshot_id_again: &'q str,
    base_snapshot_id_again: &'q str,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    query
        .bind(base_snapshot_id)
        .bind(current_snapshot_id)
        .bind(current_snapshot_id_again)
        .bind(base_snapshot_id_again)
}

fn changes_cte(base_table: &str) -> String {
    format!(
        r#"
        WITH changes AS (
          SELECT
            c.path AS path, c.kind AS kind, c.size AS size, c.mtime_ms AS mtime_ms, c.mode AS mode,
            b.kind AS baseline_kind, b.size AS baseline_size, b.mtime_ms AS baseline_mtime_ms, b.mode AS baseline_mode,
            CASE
              WHEN b.path IS NULL THEN 'added'
              WHEN c.kind != b.kind THEN 'changed'
              WHEN c.kind = 'file' AND (c.size != b.size OR c.mtime_ms != b.mtime_ms OR c.mode != b.mode) THEN 'changed'
              ELSE 'unchanged'
            END AS change
          FROM files c
          LEFT JOIN {base_table} b ON b.snapshot_id = ? AND b.path = c.path
          WHERE c.snapshot_id = ?
          UNION ALL
          SELECT
            b.path AS path, b.kind AS kind, b.size AS size, b.mtime_ms AS mtime_ms, b.mode AS mode,
            b.kind AS baseline_kind, b.size AS baseline_size, b.mtime_ms AS baseline_mtime_ms, b.mode AS baseline_mode,
            'deleted' AS change
          FROM {base_table} b
          LEFT JOIN files c ON c.snapshot_id = ? AND c.path = b.path
          WHERE b.snapshot_id = ? AND c.path IS NULL
        )
        "#
    )
}

async fn fetch_all_files(
    connection: &mut SqliteConnection,
    request: &FileInspectionRequest,
    after: Option<&str>,
) -> Result<Vec<FileEntry>> {
    let (tree_filter, tree_binds) = tree_filter(request.presentation, request.parent.as_deref());
    let query_text = normalize_query(request.query.as_deref());
    let sql = format!(
        r#"
        SELECT path, kind, size, mtime_ms, mode
        FROM files
        WHERE snapshot_id = ?
          AND ({tree_filter})
          AND (? = '' OR instr(lower(path), lower(?)) > 0)
          AND (? = '' OR path > ? COLLATE BINARY)
        ORDER BY path COLLATE BINARY
        LIMIT ?
        "#
    );
    let query = sqlx::query(&sql).bind(&request.snapshot_id);
    let query = bind_tree_filter(query, tree_binds);
    let query = query
        .bind(&query_text)
        .bind(&query_text)
        .bind(after.unwrap_or_default())
        .bind(after.unwrap_or_default())
        .bind(i64::from(request.limit) + 1);
    Ok(query
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|row| row_to_file_entry(row, "unchanged", false, None, None))
        .collect())
}

async fn fetch_first_snapshot_changes(
    connection: &mut SqliteConnection,
    request: &FileInspectionRequest,
    after: Option<&str>,
) -> Result<Vec<FileEntry>> {
    let (tree_filter, tree_binds) = tree_filter(request.presentation, request.parent.as_deref());
    let query_text = normalize_query(request.query.as_deref());
    let sql = format!(
        r#"
        SELECT f.path, f.kind, f.size, f.mtime_ms, f.mode,
               CASE WHEN f.kind = 'dir' THEN COALESCE((
                 SELECT COUNT(*) FROM files descendant
                 WHERE descendant.snapshot_id = f.snapshot_id
                   AND descendant.path LIKE f.path || '/%'
               ), 0) ELSE 0 END AS descendant_added,
               0 AS descendant_deleted,
               0 AS descendant_changed
        FROM files f
        WHERE snapshot_id = ?
          AND ({tree_filter})
          AND (? = '' OR instr(lower(path), lower(?)) > 0)
          AND (? = '' OR path > ? COLLATE BINARY)
        ORDER BY path COLLATE BINARY
        LIMIT ?
        "#
    );
    let query = sqlx::query(&sql).bind(&request.snapshot_id);
    let query = bind_tree_filter(query, tree_binds);
    let query = query
        .bind(&query_text)
        .bind(&query_text)
        .bind(after.unwrap_or_default())
        .bind(after.unwrap_or_default())
        .bind(i64::from(request.limit) + 1);
    let is_tree = request.presentation == FilePresentation::Tree;
    Ok(query
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|row| {
            let descendant_changes =
                (is_tree && row.get::<String, _>("kind") == "dir").then_some(ChangeCounts {
                    added: non_negative_u64(&row, "descendant_added"),
                    deleted: non_negative_u64(&row, "descendant_deleted"),
                    changed: non_negative_u64(&row, "descendant_changed"),
                });
            row_to_file_entry(row, "added", false, None, descendant_changes)
        })
        .collect())
}

async fn fetch_baseline_changes(
    connection: &mut SqliteConnection,
    request: &FileInspectionRequest,
    after: Option<&str>,
    base_snapshot_id: &str,
    base_table: &str,
) -> Result<Vec<FileEntry>> {
    let cte = changes_cte(base_table);
    let tree = request.presentation == FilePresentation::Tree;
    let (tree_filter, tree_binds) = tree_filter(request.presentation, request.parent.as_deref());
    let query_text = normalize_query(request.query.as_deref());
    let tree_selection = if tree {
        r#"
        (change != 'unchanged'
          OR (kind = 'dir' AND EXISTS (
            SELECT 1 FROM changes descendant
            WHERE descendant.path LIKE changes.path || '/%'
              AND descendant.change != 'unchanged'
          )))
        "#
    } else {
        "change != 'unchanged'"
    };
    let descendant_columns = if tree {
        r#"
        CASE WHEN kind = 'dir' THEN COALESCE((SELECT SUM(CASE WHEN descendant.change = 'added' THEN 1 ELSE 0 END) FROM changes descendant WHERE descendant.path LIKE changes.path || '/%'), 0) ELSE 0 END AS descendant_added,
        CASE WHEN kind = 'dir' THEN COALESCE((SELECT SUM(CASE WHEN descendant.change = 'deleted' THEN 1 ELSE 0 END) FROM changes descendant WHERE descendant.path LIKE changes.path || '/%'), 0) ELSE 0 END AS descendant_deleted,
        CASE WHEN kind = 'dir' THEN COALESCE((SELECT SUM(CASE WHEN descendant.change = 'changed' THEN 1 ELSE 0 END) FROM changes descendant WHERE descendant.path LIKE changes.path || '/%'), 0) ELSE 0 END AS descendant_changed
        "#
    } else {
        "0 AS descendant_added, 0 AS descendant_deleted, 0 AS descendant_changed"
    };
    let sql = format!(
        r#"
        {cte}
        SELECT path, kind, size, mtime_ms, mode,
               baseline_kind, baseline_size, baseline_mtime_ms, baseline_mode, change,
               {descendant_columns}
        FROM changes
        WHERE ({tree_selection})
          AND ({tree_filter})
          AND (? = '' OR instr(lower(path), lower(?)) > 0)
          AND (? = '' OR path > ? COLLATE BINARY)
        ORDER BY path COLLATE BINARY
        LIMIT ?
        "#
    );
    let query = bind_changes_query(
        sqlx::query(&sql),
        base_snapshot_id,
        &request.snapshot_id,
        &request.snapshot_id,
        base_snapshot_id,
    );
    let query = bind_tree_filter(query, tree_binds);
    let query = query
        .bind(&query_text)
        .bind(&query_text)
        .bind(after.unwrap_or_default())
        .bind(after.unwrap_or_default())
        .bind(i64::from(request.limit) + 1);
    Ok(query
        .fetch_all(&mut *connection)
        .await?
        .into_iter()
        .map(|row| {
            let change: String = row.get("change");
            let is_ancestor_context = tree && change == "unchanged";
            let baseline = match change.as_str() {
                "added" | "unchanged" => None,
                _ => Some(FileMetadata {
                    kind: row.get("baseline_kind"),
                    size: non_negative_u64(&row, "baseline_size"),
                    mtime_ms: row.get("baseline_mtime_ms"),
                    mode: row.get("baseline_mode"),
                }),
            };
            let descendant_changes =
                (tree && row.get::<String, _>("kind") == "dir").then_some(ChangeCounts {
                    added: non_negative_u64(&row, "descendant_added"),
                    deleted: non_negative_u64(&row, "descendant_deleted"),
                    changed: non_negative_u64(&row, "descendant_changed"),
                });
            row_to_file_entry(
                row,
                &change,
                is_ancestor_context,
                baseline,
                descendant_changes,
            )
        })
        .collect())
}

fn row_to_file_entry(
    row: sqlx::sqlite::SqliteRow,
    change: &str,
    is_ancestor_context: bool,
    baseline: Option<FileMetadata>,
    descendant_changes: Option<ChangeCounts>,
) -> FileEntry {
    let path: String = row.get("path");
    let name = path.rsplit('/').next().unwrap_or(path.as_str()).to_string();
    FileEntry {
        path,
        name,
        kind: row.get("kind"),
        change: change.to_string(),
        is_ancestor_context,
        size: non_negative_u64(&row, "size"),
        mtime_ms: row.get("mtime_ms"),
        mode: row.get("mode"),
        baseline,
        descendant_changes,
    }
}

enum TreeBinds<'a> {
    List,
    Tree { parent: &'a str },
}

fn tree_filter<'a>(
    presentation: FilePresentation,
    parent: Option<&'a str>,
) -> (&'static str, TreeBinds<'a>) {
    match presentation {
        FilePresentation::List => ("1 = 1", TreeBinds::List),
        FilePresentation::Tree => (
            "((? = '' AND instr(path, '/') = 0) OR (? <> '' AND path LIKE ? || '/%' AND instr(substr(path, length(?) + 2), '/') = 0))",
            TreeBinds::Tree {
                parent: parent.unwrap_or_default(),
            },
        ),
    }
}

fn bind_tree_filter<'q>(
    query: sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>>,
    binds: TreeBinds<'q>,
) -> sqlx::query::Query<'q, sqlx::Sqlite, sqlx::sqlite::SqliteArguments<'q>> {
    match binds {
        TreeBinds::List => query,
        TreeBinds::Tree { parent } => query.bind(parent).bind(parent).bind(parent).bind(parent),
    }
}

fn validate_page_size(limit: u16) -> Result<()> {
    if !(1..=MAX_PAGE_SIZE).contains(&limit) {
        return Err(SnapshotInspectionError::InvalidArgument {
            message: format!("limit must be between 1 and {MAX_PAGE_SIZE}"),
        });
    }
    Ok(())
}

fn validate_relative_path(path: Option<&str>, field: &str) -> Result<()> {
    let Some(path) = path else {
        return Ok(());
    };
    if path.starts_with('/')
        || path.ends_with('/')
        || path.contains("//")
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(SnapshotInspectionError::InvalidArgument {
            message: format!("{field} must be a normalized relative path"),
        });
    }
    Ok(())
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct InspectionCursor {
    version: u8,
    resource: String,
    snapshot_id: String,
    presentation: Option<String>,
    scope: Option<String>,
    parent: Option<String>,
    query: String,
    #[serde(default)]
    changes_only: bool,
    limit: u16,
    after: String,
}

fn decode_file_cursor(request: &FileInspectionRequest) -> Result<Option<String>> {
    let Some(cursor) = request.cursor.as_deref() else {
        return Ok(None);
    };
    let decoded = decode_cursor(cursor)?;
    let expected = file_cursor(request, String::new());
    if decoded.version != expected.version
        || decoded.resource != expected.resource
        || decoded.snapshot_id != expected.snapshot_id
        || decoded.presentation != expected.presentation
        || decoded.scope != expected.scope
        || decoded.parent != expected.parent
        || decoded.query != expected.query
        || decoded.changes_only != expected.changes_only
        || decoded.limit != expected.limit
        || decoded.after.is_empty()
    {
        return Err(SnapshotInspectionError::InvalidCursor {
            message: "cursor does not match the file inspection request".to_string(),
        });
    }
    Ok(Some(decoded.after))
}

fn decode_block_cursor(request: &BlockInspectionRequest) -> Result<Option<String>> {
    let Some(cursor) = request.cursor.as_deref() else {
        return Ok(None);
    };
    let decoded = decode_cursor(cursor)?;
    let expected = block_cursor(request, String::new());
    if decoded.version != expected.version
        || decoded.resource != expected.resource
        || decoded.snapshot_id != expected.snapshot_id
        || decoded.presentation != expected.presentation
        || decoded.scope != expected.scope
        || decoded.parent != expected.parent
        || decoded.query != expected.query
        || decoded.changes_only != expected.changes_only
        || decoded.limit != expected.limit
        || decoded.after.is_empty()
    {
        return Err(SnapshotInspectionError::InvalidCursor {
            message: "cursor does not match the block inspection request".to_string(),
        });
    }
    Ok(Some(decoded.after))
}

fn decode_storage_cursor(request: &StorageInspectionRequest) -> Result<Option<String>> {
    let Some(cursor) = request.cursor.as_deref() else {
        return Ok(None);
    };
    let decoded = decode_cursor(cursor)?;
    let expected = storage_cursor(request, String::new());
    if decoded.version != expected.version
        || decoded.resource != expected.resource
        || decoded.snapshot_id != expected.snapshot_id
        || decoded.query != expected.query
        || decoded.scope != expected.scope
        || decoded.limit != expected.limit
        || decoded.after.is_empty()
    {
        return Err(SnapshotInspectionError::InvalidCursor {
            message: "cursor does not match the storage inspection request".to_string(),
        });
    }
    Ok(Some(decoded.after))
}

fn decode_storage_blocks_cursor(
    request: &StorageBlocksInspectionRequest,
) -> Result<Option<String>> {
    let Some(cursor) = request.cursor.as_deref() else {
        return Ok(None);
    };
    let decoded = decode_cursor(cursor)?;
    let expected = storage_blocks_cursor(request, String::new());
    if decoded.version != expected.version
        || decoded.resource != expected.resource
        || decoded.snapshot_id != expected.snapshot_id
        || decoded.parent != expected.parent
        || decoded.limit != expected.limit
        || decoded.after.is_empty()
    {
        return Err(SnapshotInspectionError::InvalidCursor {
            message: "cursor does not match the storage block inspection request".to_string(),
        });
    }
    Ok(Some(decoded.after))
}

fn decode_cursor(cursor: &str) -> Result<InspectionCursor> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(cursor)
        .map_err(|_| SnapshotInspectionError::InvalidCursor {
            message: "cursor is not valid base64url".to_string(),
        })?;
    serde_json::from_slice(&bytes).map_err(|_| SnapshotInspectionError::InvalidCursor {
        message: "cursor has an unsupported payload".to_string(),
    })
}

fn encode_file_cursor(request: &FileInspectionRequest, after: String) -> String {
    encode_cursor(file_cursor(request, after))
}

fn encode_block_cursor(request: &BlockInspectionRequest, after: String) -> String {
    encode_cursor(block_cursor(request, after))
}

fn encode_storage_cursor(request: &StorageInspectionRequest, after: String) -> String {
    encode_cursor(storage_cursor(request, after))
}

fn encode_storage_blocks_cursor(request: &StorageBlocksInspectionRequest, after: String) -> String {
    encode_cursor(storage_blocks_cursor(request, after))
}

fn encode_cursor(cursor: InspectionCursor) -> String {
    let bytes = serde_json::to_vec(&cursor).expect("inspection cursor serializes");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn file_cursor(request: &FileInspectionRequest, after: String) -> InspectionCursor {
    InspectionCursor {
        version: CURSOR_VERSION,
        resource: "files".to_string(),
        snapshot_id: request.snapshot_id.clone(),
        presentation: Some(
            match request.presentation {
                FilePresentation::Tree => "tree",
                FilePresentation::List => "list",
            }
            .to_string(),
        ),
        scope: Some(
            match request.scope {
                FileScope::All => "all",
                FileScope::Changes => "changes",
            }
            .to_string(),
        ),
        parent: request.parent.clone(),
        query: normalize_query(request.query.as_deref()),
        changes_only: false,
        limit: request.limit,
        after,
    }
}

fn block_cursor(request: &BlockInspectionRequest, after: String) -> InspectionCursor {
    InspectionCursor {
        version: CURSOR_VERSION,
        resource: "blocks".to_string(),
        snapshot_id: request.snapshot_id.clone(),
        presentation: None,
        scope: None,
        parent: None,
        query: normalize_query(request.query.as_deref()),
        changes_only: request.changes_only,
        limit: request.limit,
        after,
    }
}

fn storage_cursor(request: &StorageInspectionRequest, after: String) -> InspectionCursor {
    InspectionCursor {
        version: CURSOR_VERSION,
        resource: "storage".to_string(),
        snapshot_id: request.snapshot_id.clone(),
        presentation: None,
        scope: request.kind.clone(),
        parent: None,
        query: normalize_query(request.query.as_deref()),
        changes_only: false,
        limit: request.limit,
        after,
    }
}

fn storage_blocks_cursor(
    request: &StorageBlocksInspectionRequest,
    after: String,
) -> InspectionCursor {
    InspectionCursor {
        version: CURSOR_VERSION,
        resource: "storage-blocks".to_string(),
        snapshot_id: request.snapshot_id.clone(),
        presentation: None,
        scope: None,
        parent: Some(request.storage_id.clone()),
        query: String::new(),
        changes_only: false,
        limit: request.limit,
        after,
    }
}

fn normalize_query(query: Option<&str>) -> String {
    query.unwrap_or_default().trim().to_lowercase()
}

fn non_negative_u64(row: &sqlx::sqlite::SqliteRow, column: &str) -> u64 {
    row.try_get::<i64, _>(column).unwrap_or(0).max(0) as u64
}
