use std::path::{Path, PathBuf};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sqlx::{Row, SqliteConnection};

use crate::{Error, index_db};

pub const MAX_PAGE_SIZE: u16 = 500;
const CURSOR_VERSION: u8 = 1;

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
}

impl SnapshotInspector {
    pub fn new(endpoint_db_path: impl Into<PathBuf>, filemap_dir: impl Into<PathBuf>) -> Self {
        Self {
            endpoint_db_path: endpoint_db_path.into(),
            filemap_dir: filemap_dir.into(),
        }
    }

    pub fn endpoint_db_path(&self) -> &Path {
        &self.endpoint_db_path
    }

    pub fn filemap_path(&self, snapshot_id: &str) -> PathBuf {
        self.filemap_dir.join(format!("{snapshot_id}.sqlite"))
    }

    pub async fn summary(&self, snapshot_id: &str) -> Result<SnapshotSummary> {
        let context = self.resolve_context(snapshot_id).await?;
        let pool = index_db::open_existing_index_db(&context.current_path).await?;
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

        let pool = index_db::open_existing_index_db(&context.current_path).await?;
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
        let after = decode_block_cursor(&request)?;
        let context = self.resolve_context(&request.snapshot_id).await?;
        let pool = index_db::open_existing_index_db(&context.current_path).await?;
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
        let endpoint_pool = index_db::open_existing_index_db(&self.endpoint_db_path).await?;
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotMetadata {
    pub snapshot_id: String,
    pub created_at: String,
    pub source_path: String,
    pub label: String,
    pub base_snapshot_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSummary {
    pub snapshot: SnapshotMetadata,
    pub availability: DifferenceAvailability,
    pub files: FileCounts,
    pub changes: ChangeSummary,
    pub blocks: BlockCounts,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCounts {
    pub entries: u64,
    pub regular_files: u64,
    pub directories: u64,
    pub symlinks: u64,
    pub bytes: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeSummary {
    pub state: String,
    pub added: u64,
    pub deleted: u64,
    pub changed: u64,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePage {
    pub entries: Vec<FileEntry>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize)]
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

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FileMetadata {
    pub kind: String,
    pub size: u64,
    pub mtime_ms: i64,
    pub mode: i64,
}

#[derive(Debug, Serialize)]
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
    pub referencing_files: u64,
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
        || decoded.limit != expected.limit
        || decoded.after.is_empty()
    {
        return Err(SnapshotInspectionError::InvalidCursor {
            message: "cursor does not match the block inspection request".to_string(),
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
