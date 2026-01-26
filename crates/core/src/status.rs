use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub fn now_unix_ms() -> u64 {
    static LAST_UNIX_MS: AtomicU64 = AtomicU64::new(0);

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let mut prev = LAST_UNIX_MS.load(Ordering::Relaxed);
    loop {
        let next = now.max(prev);
        match LAST_UNIX_MS.compare_exchange_weak(prev, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(current) => prev = current,
        }
    }
}

pub fn status_json_path(data_dir: &Path) -> PathBuf {
    data_dir.join("status").join("status.json")
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rate {
    pub bytes_per_second: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Counter {
    pub bytes: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Progress {
    pub phase: String,
    pub files_total: Option<u64>,
    pub files_done: Option<u64>,
    pub chunks_total: Option<u64>,
    pub chunks_done: Option<u64>,
    pub bytes_read: Option<u64>,
    pub bytes_uploaded: Option<u64>,
    pub bytes_deduped: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetRunSummary {
    pub finished_at: Option<String>,
    pub duration_seconds: Option<f64>,
    pub status: Option<String>,
    pub error_code: Option<String>,
    pub files_indexed: Option<u64>,
    pub bytes_uploaded: Option<u64>,
    pub bytes_deduped: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSource {
    pub kind: String, // "daemon" | "cli" | "file"
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalStatus {
    pub up: Rate,
    pub down: Rate,
    pub up_total: Counter,
    pub down_total: Counter,
    pub ui_uptime_seconds: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetState {
    pub target_id: String,
    pub label: Option<String>,
    pub source_path: String,
    pub endpoint_id: String,
    pub enabled: bool,

    pub state: String, // "idle" | "running" | "failed" | "stale"

    pub running_since: Option<u64>,

    pub up: Rate,
    pub up_total: Counter,

    pub progress: Option<Progress>,
    pub last_run: Option<TargetRunSummary>,

    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusSnapshot {
    #[serde(rename = "type")]
    pub type_: String, // "status.snapshot"
    pub schema_version: u32,
    pub generated_at: u64,
    pub source: StatusSource,
    pub global: GlobalStatus,
    pub targets: Vec<TargetState>,

    #[serde(default)]
    pub extra: BTreeMap<String, serde_json::Value>,
}

pub fn read_status_snapshot_json(path: &Path) -> std::io::Result<StatusSnapshot> {
    let mut f = File::open(path)?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    let snap: StatusSnapshot = serde_json::from_slice(&buf)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    Ok(snap)
}

#[derive(Debug, Clone, Copy)]
pub struct StatusWriteOptions {
    pub fsync_file: bool,
    pub fsync_dir: bool,
}

impl Default for StatusWriteOptions {
    fn default() -> Self {
        Self {
            fsync_file: true,
            fsync_dir: true,
        }
    }
}

pub fn write_status_snapshot_json_atomic(
    path: &Path,
    snapshot: &StatusSnapshot,
) -> std::io::Result<()> {
    write_status_snapshot_json_atomic_with_options(path, snapshot, StatusWriteOptions::default())
}

pub fn write_status_snapshot_json_atomic_with_options(
    path: &Path,
    snapshot: &StatusSnapshot,
    options: StatusWriteOptions,
) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension(format!("json.tmp.{}", std::process::id()));
    let data = serde_json::to_vec(snapshot)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)?;
    f.write_all(&data)?;
    if options.fsync_file {
        f.sync_all()?;
    }
    drop(f);

    std::fs::rename(&tmp, path)?;

    // Best-effort directory sync (ignored on platforms where it isn't supported).
    if options.fsync_dir
        && let Some(parent) = path.parent()
        && let Ok(dir) = File::open(parent)
    {
        let _ = dir.sync_all();
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_status_snapshot_json_atomic() {
        let dir = tempfile::tempdir().unwrap();
        let path = status_json_path(dir.path());

        let snapshot = StatusSnapshot {
            type_: "status.snapshot".to_string(),
            schema_version: 1,
            generated_at: 123,
            source: StatusSource {
                kind: "daemon".to_string(),
                detail: Some("test".to_string()),
            },
            global: GlobalStatus {
                up: Rate {
                    bytes_per_second: None,
                },
                down: Rate {
                    bytes_per_second: None,
                },
                up_total: Counter { bytes: None },
                down_total: Counter { bytes: None },
                ui_uptime_seconds: None,
            },
            targets: vec![TargetState {
                target_id: "t1".to_string(),
                label: None,
                source_path: "/tmp".to_string(),
                endpoint_id: "ep".to_string(),
                enabled: true,
                state: "idle".to_string(),
                running_since: None,
                up: Rate {
                    bytes_per_second: None,
                },
                up_total: Counter { bytes: None },
                progress: None,
                last_run: None,
                extra: Default::default(),
            }],
            extra: Default::default(),
        };

        write_status_snapshot_json_atomic(&path, &snapshot).unwrap();
        let got = read_status_snapshot_json(&path).unwrap();

        assert_eq!(got.type_, "status.snapshot");
        assert_eq!(got.schema_version, 1);
        assert_eq!(got.targets.len(), 1);
        assert_eq!(got.targets[0].target_id, "t1");
    }

    #[test]
    fn deserializes_with_missing_optional_fields() {
        let json = r#"
{
  "type": "status.snapshot",
  "schemaVersion": 1,
  "generatedAt": 1700000000000,
  "source": { "kind": "daemon", "detail": null },
  "global": {
    "up": { "bytesPerSecond": null },
    "down": { "bytesPerSecond": null },
    "upTotal": { "bytes": null },
    "downTotal": { "bytes": null }
  },
  "targets": [
    {
      "targetId": "t1",
      "label": null,
      "sourcePath": "/Volumes/SSD",
      "endpointId": "ep_default",
      "enabled": true,
      "state": "idle",
      "up": { "bytesPerSecond": null },
      "upTotal": { "bytes": null }
    }
  ]
}
"#;

        let snap: StatusSnapshot = serde_json::from_str(json).unwrap();
        assert_eq!(snap.targets[0].target_id, "t1");
        assert!(snap.targets[0].running_since.is_none());
    }
}
