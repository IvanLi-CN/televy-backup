use serde::{Deserialize, Serialize};

pub mod mount_helper;

pub const PROTOCOL_VERSION: u32 = 2;
pub const MOUNT_HELPER_PROTOCOL_VERSION: u32 = 1;
pub const MOUNT_HELPER_LABEL: &str = "com.ivan.televybackup.snapshot-mount-helper";
pub const DEFAULT_MOUNT_HELPER_SOCKET: &str =
    "/private/var/run/televybackup/snapshot-mount-helper.sock";
pub const DEFAULT_MOUNT_HELPER_JOURNAL: &str =
    "/var/db/televybackup/snapshot-mount-helper/journal.sqlite";
pub const DEFAULT_SOCKET_PATH: &str =
    "~/Library/Application Support/TelevyBackup/snapshot-access/access.sock";
pub const DEFAULT_JOURNAL_PATH: &str =
    "~/Library/Application Support/TelevyBackup/snapshot-access/journal.sqlite";
pub const CONFIG_DIR_ENV: &str = "TELEVYBACKUP_CONFIG_DIR";
pub const DATA_DIR_ENV: &str = "TELEVYBACKUP_DATA_DIR";
pub const MIN_FREE_BYTES: u64 = 10 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub version: u32,
    pub request_id: String,
    #[serde(flatten)]
    pub method: Method,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum Method {
    Status,
    ProbeVolume {
        target_id: String,
    },
    AcquireLease {
        target_id: String,
        expected_volume_uuid: String,
        run_id: String,
    },
    ScanPage {
        lease_id: String,
        cursor: Option<String>,
        limit: u16,
    },
    OpenReadStream {
        lease_id: String,
        relative_path: String,
    },
    ReadStream {
        stream_id: String,
        max_bytes: u32,
    },
    CloseReadStream {
        stream_id: String,
    },
    ReleaseLease {
        lease_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub version: u32,
    pub request_id: String,
    pub ok: bool,
    pub code: Option<String>,
    pub message: Option<String>,
    pub result: Option<ResponseResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseResult {
    Status(StatusResult),
    Probe(ProbeResult),
    Lease(LeaseResult),
    ScanPage(ScanPageResult),
    ReadStream(ReadStreamResult),
    Closed,
    Released(ReleaseResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusResult {
    pub active_leases: u32,
    pub pending_cleanup: u32,
    pub access_app_version: String,
    pub fda_ready: bool,
    #[serde(default)]
    pub mount_helper_reachable: bool,
    #[serde(default)]
    pub mount_helper_version: Option<String>,
    #[serde(default)]
    pub mount_helper_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountSnapshotRef {
    pub device_identifier: String,
    pub uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountRequest {
    pub version: u32,
    pub request_id: String,
    #[serde(flatten)]
    pub method: MountMethod,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum MountMethod {
    Status,
    Mount {
        uid: u32,
        lease_id: String,
        volume_uuid: String,
        device_identifier: String,
        snapshot_uuid: String,
        snapshot_name: String,
        source_mount_point: String,
        mount_root: String,
        snapshot_manifest: Vec<MountSnapshotRef>,
    },
    Release {
        uid: u32,
        lease_id: String,
    },
    Cleanup {
        uid: u32,
        cleanup_id: String,
        snapshot_manifest: Vec<MountSnapshotRef>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountResponse {
    pub version: u32,
    pub request_id: String,
    pub ok: bool,
    pub code: Option<String>,
    pub message: Option<String>,
    pub result: Option<MountResponseResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MountResponseResult {
    Status(MountStatusResult),
    Mounted(MountLeaseResult),
    Released(MountReleaseResult),
    Cleaned(MountReleaseResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MountStatusResult {
    pub helper_version: String,
    pub active_mounts: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountLeaseResult {
    pub lease_id: String,
    pub mount_root: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MountReleaseResult {
    pub lease_id: String,
    pub cleanup_state: String,
}

impl MountResponse {
    pub fn ok(request_id: impl Into<String>, result: MountResponseResult) -> Self {
        Self {
            version: MOUNT_HELPER_PROTOCOL_VERSION,
            request_id: request_id.into(),
            ok: true,
            code: None,
            message: None,
            result: Some(result),
        }
    }

    pub fn error(
        request_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            version: MOUNT_HELPER_PROTOCOL_VERSION,
            request_id: request_id.into(),
            ok: false,
            code: Some(code.into()),
            message: Some(message.into()),
            result: None,
        }
    }
}

pub fn validate_mount_request(request: &MountRequest) -> Result<(), &'static str> {
    if request.version != MOUNT_HELPER_PROTOCOL_VERSION {
        return Err("unsupported mount helper protocol version");
    }
    if request.request_id.is_empty() || request.request_id.len() > 128 {
        return Err("invalid mount helper request id");
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub volume_uuid: String,
    pub device_identifier: String,
    pub mount_point: String,
    pub filesystem: String,
    pub free_bytes: u64,
    pub snapshot_supported: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceEntry {
    pub relative_path: String,
    pub kind: String,
    pub size: i64,
    pub mtime_ms: i64,
    pub mode: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanPageResult {
    pub entries: Vec<SourceEntry>,
    pub next_cursor: Option<String>,
    pub ignore_rule_files: u64,
    pub ignore_invalid_rules: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LeaseResult {
    pub lease_id: String,
    pub uid: u32,
    pub volume_uuid: String,
    pub snapshot_uuid: String,
    pub snapshot_name: String,
    pub source_path: String,
    pub snapshot_created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadStreamResult {
    pub stream_id: String,
    pub eof: bool,
    pub bytes_base64: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseResult {
    pub lease_id: String,
    pub cleanup_state: String,
}

impl Response {
    pub fn ok(request_id: impl Into<String>, result: ResponseResult) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            ok: true,
            code: None,
            message: None,
            result: Some(result),
        }
    }

    pub fn error(
        request_id: impl Into<String>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            request_id: request_id.into(),
            ok: false,
            code: Some(code.into()),
            message: Some(message.into()),
            result: None,
        }
    }
}

pub fn validate_request(request: &Request) -> Result<(), &'static str> {
    if request.version != PROTOCOL_VERSION {
        return Err("unsupported protocol version");
    }
    if request.request_id.is_empty() || request.request_id.len() > 128 {
        return Err("invalid request id");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_with_method_tag() {
        let request = Request {
            version: PROTOCOL_VERSION,
            request_id: "r1".to_string(),
            method: Method::AcquireLease {
                expected_volume_uuid: "A".to_string(),
                run_id: "run".to_string(),
                target_id: "target".to_string(),
            },
        };
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(decoded.method, Method::AcquireLease { .. }));
        assert!(validate_request(&decoded).is_ok());
    }
}
