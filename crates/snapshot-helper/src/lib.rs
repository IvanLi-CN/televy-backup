use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;
pub const DEFAULT_SOCKET_PATH: &str = "/var/run/com.ivan.televybackup.snapshot-helper.sock";
pub const DEFAULT_JOURNAL_PATH: &str =
    "/var/db/com.ivan.televybackup.snapshot-helper/journal.sqlite";
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
    Probe {
        source_path: String,
        expected_volume_uuid: Option<String>,
    },
    AcquireLease {
        source_path: String,
        expected_volume_uuid: String,
        run_id: String,
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
    Released(ReleaseResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StatusResult {
    pub active_leases: u32,
    pub pending_cleanup: u32,
    pub helper_version: String,
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
pub struct LeaseResult {
    pub lease_id: String,
    pub uid: u32,
    pub volume_uuid: String,
    pub snapshot_uuid: String,
    pub snapshot_name: String,
    pub mount_root: String,
    pub source_relative_path: String,
    pub snapshot_created_at: String,
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
                source_path: "/Users/test/Documents".to_string(),
                expected_volume_uuid: "A".to_string(),
                run_id: "run".to_string(),
            },
        };
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: Request = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(decoded.method, Method::AcquireLease { .. }));
        assert!(validate_request(&decoded).is_ok());
    }
}
