use std::path::PathBuf;
use std::sync::Arc;

use televy_backup_core::snapshot_browsing::{SnapshotBrowseCache, SnapshotContentReader};

#[tokio::test]
#[ignore = "requires an operator-provided local catalog and filemaps"]
async fn supplied_catalog_is_browseable_readonly() {
    let endpoint_db = PathBuf::from(
        std::env::var_os("TELEVYBACKUP_BROWSE_ENDPOINT_DB").expect("endpoint DB path is required"),
    );
    let filemap_dir = PathBuf::from(
        std::env::var_os("TELEVYBACKUP_BROWSE_FILEMAP_DIR")
            .expect("filemap directory path is required"),
    );
    let source_path =
        std::env::var("TELEVYBACKUP_BROWSE_SOURCE_PATH").expect("target source path is required");
    let temp = tempfile::tempdir().unwrap();
    let reader = SnapshotContentReader::new_cached(
        endpoint_db,
        filemap_dir,
        "offline.catalog.check",
        Arc::new(SnapshotBrowseCache::new(temp.path().join("cache"), 1024)),
    );

    let snapshots = reader.list_snapshots(&source_path).await.unwrap();
    assert!(!snapshots.is_empty(), "target has no retained snapshots");
    for snapshot in &snapshots {
        reader
            .unavailable_entries(&snapshot.snapshot_id)
            .await
            .unwrap();
        reader
            .list_children(&snapshot.snapshot_id, "")
            .await
            .unwrap();
    }
    println!("validated_snapshots={}", snapshots.len());
}
