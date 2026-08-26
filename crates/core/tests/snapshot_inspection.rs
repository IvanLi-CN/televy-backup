use std::path::Path;

use sqlx::SqlitePool;
use televy_backup_core::{
    index_db,
    snapshot_inspection::{
        BlockInspectionRequest, FileInspectionRequest, FilePresentation, FileScope,
        SnapshotInspectionError, SnapshotInspector,
    },
};
use tempfile::TempDir;

#[derive(Clone, Copy)]
struct FileSpec {
    path: &'static str,
    kind: &'static str,
    size: i64,
    mtime_ms: i64,
    mode: i64,
    chunk: Option<&'static str>,
}

async fn insert_snapshot(pool: &SqlitePool, snapshot_id: &str, base_snapshot_id: Option<&str>) {
    sqlx::query(
        "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, '2026-08-27T00:00:00Z', '/source', 'Project', ?)",
    )
    .bind(snapshot_id)
    .bind(base_snapshot_id)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_filemap(
    path: &Path,
    snapshot_id: &str,
    base_snapshot_id: Option<&str>,
    files: &[FileSpec],
) {
    let pool = index_db::open_index_db(path).await.unwrap();
    insert_snapshot(&pool, snapshot_id, base_snapshot_id).await;
    for (index, file) in files.iter().enumerate() {
        let file_id = format!("{snapshot_id}-file-{index}");
        sqlx::query(
            "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(&file_id)
        .bind(snapshot_id)
        .bind(file.path)
        .bind(file.size)
        .bind(file.mtime_ms)
        .bind(file.mode)
        .bind(file.kind)
        .execute(&pool)
        .await
        .unwrap();
        if let Some(hash) = file.chunk {
            sqlx::query(
                "INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES (?, 16, 'blake3', 'xchacha20poly1305', '2026-08-27T00:00:00Z')",
            )
            .bind(hash)
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len) VALUES (?, 0, ?, 0, 16)",
            )
            .bind(&file_id)
            .bind(hash)
            .execute(&pool)
            .await
            .unwrap();
        }
    }
}

async fn endpoint_with_filemaps(temp: &TempDir) -> (std::path::PathBuf, std::path::PathBuf) {
    let endpoint_path = temp.path().join("endpoint.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    std::fs::create_dir_all(&filemap_dir).unwrap();
    let endpoint = index_db::open_index_db(&endpoint_path).await.unwrap();
    insert_snapshot(&endpoint, "base", None).await;
    insert_snapshot(&endpoint, "current", Some("base")).await;
    drop(endpoint);

    seed_filemap(
        &filemap_dir.join("base.sqlite"),
        "base",
        None,
        &[
            FileSpec {
                path: "docs",
                kind: "dir",
                size: 0,
                mtime_ms: 1,
                mode: 0o755,
                chunk: None,
            },
            FileSpec {
                path: "docs/changed.txt",
                kind: "file",
                size: 10,
                mtime_ms: 1,
                mode: 0o644,
                chunk: Some("base-changed"),
            },
            FileSpec {
                path: "docs/deleted.txt",
                kind: "file",
                size: 7,
                mtime_ms: 1,
                mode: 0o644,
                chunk: Some("base-deleted"),
            },
            FileSpec {
                path: "oldkind",
                kind: "symlink",
                size: 0,
                mtime_ms: 1,
                mode: 0o777,
                chunk: None,
            },
            FileSpec {
                path: "same.txt",
                kind: "file",
                size: 4,
                mtime_ms: 1,
                mode: 0o644,
                chunk: Some("base-same"),
            },
        ],
    )
    .await;
    seed_filemap(
        &filemap_dir.join("current.sqlite"),
        "current",
        Some("base"),
        &[
            FileSpec {
                path: "docs",
                kind: "dir",
                size: 0,
                mtime_ms: 1,
                mode: 0o755,
                chunk: None,
            },
            FileSpec {
                path: "docs/added.txt",
                kind: "file",
                size: 6,
                mtime_ms: 2,
                mode: 0o644,
                chunk: Some("shared-current"),
            },
            FileSpec {
                path: "docs/changed.txt",
                kind: "file",
                size: 11,
                mtime_ms: 2,
                mode: 0o644,
                chunk: Some("shared-current"),
            },
            FileSpec {
                path: "oldkind",
                kind: "file",
                size: 2,
                mtime_ms: 2,
                mode: 0o644,
                chunk: Some("kind-current"),
            },
            FileSpec {
                path: "same.txt",
                kind: "file",
                size: 4,
                mtime_ms: 1,
                mode: 0o644,
                chunk: Some("same-current"),
            },
        ],
    )
    .await;
    (endpoint_path, filemap_dir)
}

fn file_request(
    snapshot_id: &str,
    presentation: FilePresentation,
    scope: FileScope,
    cursor: Option<String>,
    limit: u16,
) -> FileInspectionRequest {
    FileInspectionRequest {
        snapshot_id: snapshot_id.to_string(),
        presentation,
        scope,
        parent: None,
        query: None,
        cursor,
        limit,
    }
}

#[tokio::test]
async fn current_filemaps_report_direct_baseline_changes_blocks_and_request_bound_cursors() {
    let temp = TempDir::new().unwrap();
    let (endpoint_path, filemap_dir) = endpoint_with_filemaps(&temp).await;
    let inspector = SnapshotInspector::new(endpoint_path, filemap_dir);

    let summary = inspector.summary("current").await.unwrap();
    assert_eq!(summary.availability.state, "available");
    assert_eq!(summary.changes.added, 1);
    assert_eq!(summary.changes.deleted, 1);
    assert_eq!(summary.changes.changed, 2);
    assert_eq!(summary.blocks.distinct, 3);
    assert_eq!(summary.blocks.bytes, 48);

    let first_page = inspector
        .files(file_request(
            "current",
            FilePresentation::List,
            FileScope::Changes,
            None,
            2,
        ))
        .await
        .unwrap();
    assert_eq!(
        first_page
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["docs/added.txt", "docs/changed.txt"]
    );
    let cursor = first_page.next_cursor.clone().expect("next cursor");
    let second_page = inspector
        .files(file_request(
            "current",
            FilePresentation::List,
            FileScope::Changes,
            Some(cursor.clone()),
            2,
        ))
        .await
        .unwrap();
    assert_eq!(
        second_page
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        ["docs/deleted.txt", "oldkind"]
    );
    assert!(matches!(
        inspector
            .files(file_request(
                "current",
                FilePresentation::List,
                FileScope::All,
                Some(cursor),
                2
            ))
            .await,
        Err(SnapshotInspectionError::InvalidCursor { .. })
    ));

    let tree = inspector
        .files(file_request(
            "current",
            FilePresentation::Tree,
            FileScope::Changes,
            None,
            10,
        ))
        .await
        .unwrap();
    let docs = tree
        .entries
        .iter()
        .find(|entry| entry.path == "docs")
        .unwrap();
    assert!(docs.is_ancestor_context);
    let counts = docs.descendant_changes.as_ref().unwrap();
    assert_eq!((counts.added, counts.deleted, counts.changed), (1, 1, 1));

    let blocks = inspector
        .blocks(BlockInspectionRequest {
            snapshot_id: "current".to_string(),
            query: None,
            cursor: None,
            limit: 10,
        })
        .await
        .unwrap();
    let shared = blocks
        .entries
        .iter()
        .find(|entry| entry.hash == "shared-current")
        .unwrap();
    assert_eq!(shared.referencing_files, 2);
}

#[tokio::test]
async fn first_snapshot_marks_tree_entries_added_and_aggregates_descendants() {
    let temp = TempDir::new().unwrap();
    let endpoint_path = temp.path().join("endpoint.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    std::fs::create_dir_all(&filemap_dir).unwrap();
    let endpoint = index_db::open_index_db(&endpoint_path).await.unwrap();
    insert_snapshot(&endpoint, "first", None).await;
    drop(endpoint);
    seed_filemap(
        &filemap_dir.join("first.sqlite"),
        "first",
        None,
        &[
            FileSpec {
                path: "src",
                kind: "dir",
                size: 0,
                mtime_ms: 1,
                mode: 0o755,
                chunk: None,
            },
            FileSpec {
                path: "src/main.rs",
                kind: "file",
                size: 8,
                mtime_ms: 1,
                mode: 0o644,
                chunk: Some("main"),
            },
        ],
    )
    .await;
    let inspector = SnapshotInspector::new(endpoint_path, filemap_dir);

    let summary = inspector.summary("first").await.unwrap();
    assert_eq!(summary.availability.state, "firstSnapshot");
    assert_eq!(summary.changes.added, 2);
    let tree = inspector
        .files(file_request(
            "first",
            FilePresentation::Tree,
            FileScope::Changes,
            None,
            10,
        ))
        .await
        .unwrap();
    assert_eq!(tree.entries[0].change, "added");
    assert_eq!(
        tree.entries[0].descendant_changes.as_ref().unwrap().added,
        1
    );
}

#[tokio::test]
async fn missing_direct_baseline_allows_all_files_but_rejects_changes() {
    let temp = TempDir::new().unwrap();
    let endpoint_path = temp.path().join("endpoint.sqlite");
    let filemap_dir = temp.path().join("filemaps");
    std::fs::create_dir_all(&filemap_dir).unwrap();
    let endpoint = index_db::open_index_db(&endpoint_path).await.unwrap();
    insert_snapshot(&endpoint, "current", Some("expired")).await;
    drop(endpoint);
    seed_filemap(
        &filemap_dir.join("current.sqlite"),
        "current",
        Some("expired"),
        &[FileSpec {
            path: "present.txt",
            kind: "file",
            size: 2,
            mtime_ms: 1,
            mode: 0o644,
            chunk: Some("present"),
        }],
    )
    .await;
    let inspector = SnapshotInspector::new(endpoint_path, filemap_dir);

    assert_eq!(
        inspector
            .summary("current")
            .await
            .unwrap()
            .availability
            .state,
        "baselineUnavailable"
    );
    assert_eq!(
        inspector
            .files(file_request(
                "current",
                FilePresentation::List,
                FileScope::All,
                None,
                10
            ))
            .await
            .unwrap()
            .entries
            .len(),
        1
    );
    assert!(matches!(
        inspector
            .files(file_request(
                "current",
                FilePresentation::List,
                FileScope::Changes,
                None,
                10
            ))
            .await,
        Err(SnapshotInspectionError::BaselineUnavailable { .. })
    ));
}

#[tokio::test]
async fn legacy_single_index_is_used_when_no_filemap_cache_exists() {
    let temp = TempDir::new().unwrap();
    let endpoint_path = temp.path().join("index.sqlite");
    let endpoint = index_db::open_index_db(&endpoint_path).await.unwrap();
    insert_snapshot(&endpoint, "legacy", None).await;
    sqlx::query(
        "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES ('legacy-file', 'legacy', 'legacy.txt', 3, 1, 33188, 'file')",
    )
    .execute(&endpoint)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES ('legacy-block', 3, 'blake3', 'xchacha20poly1305', '2026-08-27T00:00:00Z')",
    )
    .execute(&endpoint)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len) VALUES ('legacy-file', 0, 'legacy-block', 0, 3)",
    )
    .execute(&endpoint)
    .await
    .unwrap();
    drop(endpoint);
    let inspector = SnapshotInspector::new(&endpoint_path, temp.path().join("missing-filemaps"));

    assert_eq!(
        inspector.summary("legacy").await.unwrap().blocks.distinct,
        1
    );
}

#[tokio::test]
async fn empty_legacy_single_index_snapshot_is_still_inspectable() {
    let temp = TempDir::new().unwrap();
    let endpoint_path = temp.path().join("index.sqlite");
    let endpoint = index_db::open_index_db(&endpoint_path).await.unwrap();
    insert_snapshot(&endpoint, "empty-legacy", None).await;
    drop(endpoint);
    let inspector = SnapshotInspector::new(&endpoint_path, temp.path().join("missing-filemaps"));

    let summary = inspector.summary("empty-legacy").await.unwrap();
    assert_eq!(summary.files.entries, 0);
    assert_eq!(summary.blocks.distinct, 0);
}
