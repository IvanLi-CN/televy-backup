use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use televy_backup_core::{
    BackupSource, BackupSourceEntry, Error as CoreError, Result as CoreResult,
};
use televybackup_snapshot_access::{
    LeaseResult, Method, PROTOCOL_VERSION, ProbeResult, ReadStreamResult, Request, Response,
    ResponseResult, ScanPageResult, StatusResult, VerificationResult,
};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum SnapshotClientError {
    #[error("snapshot access service unavailable: {0}")]
    Unavailable(String),
    #[error("snapshot access service rejected request ({code}): {message}")]
    Rejected { code: String, message: String },
    #[error("snapshot access protocol error: {0}")]
    Protocol(String),
    #[error("snapshot access I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("snapshot access JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct SnapshotClient {
    socket_path: PathBuf,
}

impl Default for SnapshotClient {
    fn default() -> Self {
        let data_root = std::env::var_os(televybackup_snapshot_access::DATA_DIR_ENV)
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| {
                    PathBuf::from(home).join("Library/Application Support/TelevyBackup")
                })
            })
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            socket_path: std::env::var_os("TELEVYBACKUP_SNAPSHOT_SOCKET")
                .map(PathBuf::from)
                .unwrap_or_else(|| data_root.join("snapshot-access/access.sock")),
        }
    }
}

impl SnapshotClient {
    pub fn for_data_root(data_root: &Path) -> Self {
        Self {
            socket_path: data_root.join("snapshot-access/access.sock"),
        }
    }

    #[allow(dead_code)]
    pub fn with_socket_path(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    #[allow(dead_code)]
    pub async fn ensure_running(&self) -> Result<(), SnapshotClientError> {
        self.status().await.map(|_| ())
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

    pub async fn probe_volume(&self, target_id: &str) -> Result<ProbeResult, SnapshotClientError> {
        let response = self
            .request(Method::ProbeVolume {
                target_id: target_id.to_string(),
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
        target_id: &str,
        expected_volume_uuid: &str,
        run_id: &str,
    ) -> Result<LeaseResult, SnapshotClientError> {
        let response = self
            .request(Method::AcquireLease {
                target_id: target_id.to_string(),
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

    #[allow(dead_code)]
    pub async fn scan_page(
        &self,
        lease_id: &str,
        cursor: Option<&str>,
        limit: u16,
    ) -> Result<ScanPageResult, SnapshotClientError> {
        let response = self
            .request(Method::ScanPage {
                lease_id: lease_id.to_string(),
                cursor: cursor.map(str::to_string),
                limit,
            })
            .await?;
        match response.result {
            Some(ResponseResult::ScanPage(result)) => Ok(result),
            _ => Err(SnapshotClientError::Protocol(
                "scan page response missing result".into(),
            )),
        }
    }

    #[allow(dead_code)]
    pub async fn open_read_stream(
        &self,
        lease_id: &str,
        relative_path: &str,
    ) -> Result<ReadStreamResult, SnapshotClientError> {
        let response = self
            .request(Method::OpenReadStream {
                lease_id: lease_id.to_string(),
                relative_path: relative_path.to_string(),
            })
            .await?;
        match response.result {
            Some(ResponseResult::ReadStream(result)) => Ok(result),
            _ => Err(SnapshotClientError::Protocol(
                "open read stream response missing result".into(),
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

    pub async fn verify_timepoint(
        &self,
        target_id: &str,
        confirm_probe_write: bool,
    ) -> Result<VerificationResult, SnapshotClientError> {
        let response = self
            .request(Method::VerifyTimepoint {
                target_id: target_id.to_string(),
                confirm_probe_write,
            })
            .await?;
        match response.result {
            Some(ResponseResult::Verification(result)) => Ok(result),
            _ => Err(SnapshotClientError::Protocol(
                "verification response missing result".into(),
            )),
        }
    }

    pub fn open_read_stream_blocking(
        &self,
        lease_id: &str,
        relative_path: &str,
    ) -> Result<SnapshotReadStream, SnapshotClientError> {
        let response = self.request_blocking(Method::OpenReadStream {
            lease_id: lease_id.to_string(),
            relative_path: relative_path.to_string(),
        })?;
        let ResponseResult::ReadStream(result) = response.result.ok_or_else(|| {
            SnapshotClientError::Protocol("open read stream response missing result".into())
        })?
        else {
            return Err(SnapshotClientError::Protocol(
                "open read stream returned an unexpected result".into(),
            ));
        };
        Ok(SnapshotReadStream {
            client: self.clone(),
            stream_id: result.stream_id,
            pending: Vec::new(),
            eof: false,
        })
    }

    pub fn scan_all_blocking(
        &self,
        lease_id: &str,
    ) -> Result<Vec<televybackup_snapshot_access::SourceEntry>, SnapshotClientError> {
        let mut cursor = None;
        let mut entries = Vec::new();
        loop {
            let response = self.request_blocking(Method::ScanPage {
                lease_id: lease_id.to_string(),
                cursor: cursor.clone(),
                limit: 512,
            })?;
            let ResponseResult::ScanPage(page) = response.result.ok_or_else(|| {
                SnapshotClientError::Protocol("scan page response missing result".into())
            })?
            else {
                return Err(SnapshotClientError::Protocol(
                    "scan page returned an unexpected result".into(),
                ));
            };
            entries.extend(page.entries);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        Ok(entries)
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
        decode_response(line.trim())
    }

    fn request_blocking(&self, method: Method) -> Result<Response, SnapshotClientError> {
        let request = Request {
            version: PROTOCOL_VERSION,
            request_id: Uuid::new_v4().to_string(),
            method,
        };
        let mut stream = std::os::unix::net::UnixStream::connect(&self.socket_path)
            .map_err(|error| SnapshotClientError::Unavailable(error.to_string()))?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let mut encoded = serde_json::to_vec(&request)?;
        encoded.push(b'\n');
        stream.write_all(&encoded)?;
        let mut line = String::new();
        std::io::BufReader::new(stream).read_line(&mut line)?;
        decode_response(line.trim())
    }
}

/// Core adapter for a lease owned by Snapshot Access. It exposes only logical paths and a
/// bounded Read implementation; the daemon never receives the snapshot mount path.
pub struct BrokeredSnapshotSource {
    client: SnapshotClient,
    lease_id: String,
    logical_path: PathBuf,
}

impl BrokeredSnapshotSource {
    pub fn new(client: SnapshotClient, lease_id: impl Into<String>, logical_path: PathBuf) -> Self {
        Self {
            client,
            lease_id: lease_id.into(),
            logical_path,
        }
    }
}

impl BackupSource for BrokeredSnapshotSource {
    fn logical_path(&self) -> &Path {
        &self.logical_path
    }

    fn entries(
        &self,
        _cancel: Option<&tokio_util::sync::CancellationToken>,
    ) -> CoreResult<Vec<BackupSourceEntry>> {
        self.client
            .scan_all_blocking(&self.lease_id)
            .map(|entries| {
                entries
                    .into_iter()
                    .map(|entry| BackupSourceEntry {
                        relative_path: entry.relative_path,
                        kind: entry.kind,
                        size: entry.size,
                        mtime_ms: entry.mtime_ms,
                        mode: entry.mode,
                    })
                    .collect()
            })
            .map_err(|error| CoreError::InvalidConfig {
                message: format!("snapshot access scan failed: {error}"),
            })
    }

    fn open(&self, relative_path: &str) -> CoreResult<Box<dyn Read + Send>> {
        self.client
            .open_read_stream_blocking(&self.lease_id, relative_path)
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>)
            .map_err(|error| CoreError::InvalidConfig {
                message: format!("snapshot access read failed: {error}"),
            })
    }

    fn is_brokered(&self) -> bool {
        true
    }
}

pub struct SnapshotReadStream {
    client: SnapshotClient,
    stream_id: String,
    pending: Vec<u8>,
    eof: bool,
}

impl Read for SnapshotReadStream {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        while self.pending.is_empty() && !self.eof {
            let response = self
                .client
                .request_blocking(Method::ReadStream {
                    stream_id: self.stream_id.clone(),
                    max_bytes: 1024 * 1024,
                })
                .map_err(std::io::Error::other)?;
            let ResponseResult::ReadStream(result) = response
                .result
                .ok_or_else(|| std::io::Error::other("read stream response missing result"))?
            else {
                return Err(std::io::Error::other("unexpected read stream response"));
            };
            self.pending = base64::engine::general_purpose::STANDARD
                .decode(result.bytes_base64)
                .map_err(std::io::Error::other)?;
            self.eof = result.eof;
        }
        let count = buffer.len().min(self.pending.len());
        buffer[..count].copy_from_slice(&self.pending[..count]);
        self.pending.drain(..count);
        Ok(count)
    }
}

impl Drop for SnapshotReadStream {
    fn drop(&mut self) {
        let _ = self.client.request_blocking(Method::CloseReadStream {
            stream_id: self.stream_id.clone(),
        });
    }
}

fn decode_response(line: &str) -> Result<Response, SnapshotClientError> {
    let response: Response = serde_json::from_str(line)?;
    if response.version != PROTOCOL_VERSION {
        return Err(SnapshotClientError::Protocol(
            "unsupported access app response version".into(),
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
