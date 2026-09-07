use std::path::{Path, PathBuf};

use televybackup_snapshot_helper::{
    DEFAULT_SOCKET_PATH, LeaseResult, Method, PROTOCOL_VERSION, ProbeResult, Request, Response,
    ResponseResult, StatusResult,
};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum SnapshotClientError {
    #[error("snapshot helper unavailable: {0}")]
    Unavailable(String),
    #[error("snapshot helper rejected request ({code}): {message}")]
    Rejected { code: String, message: String },
    #[error("snapshot helper protocol error: {0}")]
    Protocol(String),
    #[error("snapshot helper I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("snapshot helper JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct SnapshotClient {
    socket_path: PathBuf,
}

impl Default for SnapshotClient {
    fn default() -> Self {
        let socket_path = std::env::var_os("TELEVYBACKUP_SNAPSHOT_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET_PATH));
        Self { socket_path }
    }
}

impl SnapshotClient {
    #[allow(dead_code)]
    pub fn with_socket_path(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    pub async fn status(&self) -> Result<StatusResult, SnapshotClientError> {
        let response = self.request(Method::Status).await?;
        match response.result {
            Some(ResponseResult::Status(result)) => Ok(result),
            _ => Err(SnapshotClientError::Protocol(
                "status response missing result".into(),
            )),
        }
    }

    pub async fn probe(
        &self,
        source_path: &Path,
        expected_volume_uuid: Option<&str>,
    ) -> Result<ProbeResult, SnapshotClientError> {
        let response = self
            .request(Method::Probe {
                source_path: source_path.to_string_lossy().into_owned(),
                expected_volume_uuid: expected_volume_uuid.map(str::to_string),
            })
            .await?;
        match response.result {
            Some(ResponseResult::Probe(result)) => Ok(result),
            _ => Err(SnapshotClientError::Protocol(
                "probe response missing result".into(),
            )),
        }
    }

    pub async fn acquire_lease(
        &self,
        source_path: &Path,
        expected_volume_uuid: &str,
        run_id: &str,
    ) -> Result<LeaseResult, SnapshotClientError> {
        let response = self
            .request(Method::AcquireLease {
                source_path: source_path.to_string_lossy().into_owned(),
                expected_volume_uuid: expected_volume_uuid.to_string(),
                run_id: run_id.to_string(),
            })
            .await?;
        match response.result {
            Some(ResponseResult::Lease(result)) => Ok(result),
            _ => Err(SnapshotClientError::Protocol(
                "lease response missing result".into(),
            )),
        }
    }

    pub async fn release_lease(&self, lease_id: &str) -> Result<String, SnapshotClientError> {
        let response = self
            .request(Method::ReleaseLease {
                lease_id: lease_id.to_string(),
            })
            .await?;
        match response.result {
            Some(ResponseResult::Released(result)) => Ok(result.cleanup_state),
            _ => Err(SnapshotClientError::Protocol(
                "release response missing result".into(),
            )),
        }
    }

    async fn request(&self, method: Method) -> Result<Response, SnapshotClientError> {
        let request = Request {
            version: PROTOCOL_VERSION,
            request_id: Uuid::new_v4().to_string(),
            method,
        };
        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|error| SnapshotClientError::Unavailable(error.to_string()))?;
        let encoded = serde_json::to_vec(&request)?;
        stream.write_all(&encoded).await?;
        stream.write_all(b"\n").await?;
        stream.flush().await?;
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).await?;
        let response: Response = serde_json::from_str(line.trim())?;
        if response.version != PROTOCOL_VERSION {
            return Err(SnapshotClientError::Protocol(
                "unsupported helper response version".into(),
            ));
        }
        if !response.ok {
            return Err(SnapshotClientError::Rejected {
                code: response.code.unwrap_or_else(|| "unknown".into()),
                message: response
                    .message
                    .unwrap_or_else(|| "request rejected".into()),
            });
        }
        Ok(response)
    }
}
