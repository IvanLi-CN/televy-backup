use std::path::Path;
use std::process::Command;

use sqlx::SqlitePool;
use tempfile::TempDir;

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
    files: &[(&str, i64)],
) {
    let pool = televy_backup_core::index_db::open_index_db(path)
        .await
        .unwrap();
    insert_snapshot(&pool, snapshot_id, base_snapshot_id).await;
    for (index, (path, size)) in files.iter().enumerate() {
        let file_id = format!("{snapshot_id}-{index}");
        let hash = format!("{snapshot_id}-chunk-{index}");
        sqlx::query(
            "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES (?, ?, ?, ?, ?, 33188, 'file')",
        )
        .bind(&file_id)
        .bind(snapshot_id)
        .bind(path)
        .bind(size)
        .bind(index as i64)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES (?, ?, 'blake3', 'xchacha20poly1305', '2026-08-27T00:00:00Z')",
        )
        .bind(&hash)
        .bind(size)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len) VALUES (?, 0, ?, 0, ?)",
        )
        .bind(&file_id)
        .bind(&hash)
        .bind(size)
        .execute(&pool)
        .await
        .unwrap();
    }
}

async fn fixture() -> TempDir {
    let temp = TempDir::new().unwrap();
    let endpoint_path = temp.path().join("data/index/index.ep1.sqlite");
    std::fs::create_dir_all(endpoint_path.parent().unwrap()).unwrap();
    let endpoint = televy_backup_core::index_db::open_index_db(&endpoint_path)
        .await
        .unwrap();
    insert_snapshot(&endpoint, "base", None).await;
    insert_snapshot(&endpoint, "current", Some("base")).await;
    drop(endpoint);

    let filemap_dir = temp.path().join("data/index/filemaps/ep1");
    std::fs::create_dir_all(&filemap_dir).unwrap();
    seed_filemap(
        &filemap_dir.join("base.sqlite"),
        "base",
        None,
        &[("one.txt", 1)],
    )
    .await;
    seed_filemap(
        &filemap_dir.join("current.sqlite"),
        "current",
        Some("base"),
        &[("one.txt", 2), ("two.txt", 3)],
    )
    .await;
    temp
}

fn run_cli(temp: &TempDir, args: &[&str]) -> (std::process::ExitStatus, serde_json::Value) {
    let output = Command::new(env!("CARGO_BIN_EXE_televybackup"))
        .arg("--json")
        .arg("--config-dir")
        .arg(temp.path().join("config"))
        .arg("--data-dir")
        .arg(temp.path().join("data"))
        .args(args)
        .output()
        .unwrap();
    let stream = if output.status.success() {
        &output.stdout
    } else {
        &output.stderr
    };
    (
        output.status,
        serde_json::from_slice(stream).unwrap_or_else(|error| {
            panic!(
                "CLI did not emit JSON: {error}; stdout={}; stderr={}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            )
        }),
    )
}

#[tokio::test]
async fn inspect_commands_emit_contract_json_and_reject_mismatched_cursors() {
    let temp = fixture().await;
    let (status, summary) = run_cli(
        &temp,
        &[
            "snapshots",
            "inspect",
            "summary",
            "--snapshot-id",
            "current",
        ],
    );
    assert!(status.success());
    assert_eq!(summary["snapshot"]["snapshotId"], "current");
    assert_eq!(summary["availability"]["state"], "available");
    assert_eq!(summary["changes"]["added"], 1);
    assert_eq!(summary["changes"]["changed"], 1);

    let (status, page) = run_cli(
        &temp,
        &[
            "snapshots",
            "inspect",
            "files",
            "--snapshot-id",
            "current",
            "--presentation",
            "list",
            "--scope",
            "changes",
            "--limit",
            "1",
        ],
    );
    assert!(status.success());
    assert_eq!(page["entries"][0]["path"], "one.txt");
    let cursor = page["nextCursor"].as_str().unwrap();

    let (status, error) = run_cli(
        &temp,
        &[
            "snapshots",
            "inspect",
            "files",
            "--snapshot-id",
            "current",
            "--presentation",
            "list",
            "--scope",
            "all",
            "--limit",
            "1",
            "--cursor",
            cursor,
        ],
    );
    assert!(!status.success());
    assert_eq!(error["code"], "snapshot.inspect.invalid_cursor");

    let (status, blocks) = run_cli(
        &temp,
        &["snapshots", "inspect", "blocks", "--snapshot-id", "current"],
    );
    assert!(status.success());
    assert_eq!(blocks["entries"].as_array().unwrap().len(), 2);
}
