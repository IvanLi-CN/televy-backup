use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use televy_backup_core::{
    BackupSource, BackupSourceEntry, Error as CoreError, Result as CoreResult,
};
use televybackup_snapshot_access::{
    ACCESS_BUNDLE_ID, ACCESS_BUNDLE_RELATIVE_PATH, COMPONENT_VERSION, LeaseResult, Method,
    PROTOCOL_VERSION, ProbeResult, ReadStreamResult, Request, Response, ResponseResult,
    ScanPageResult, StatusResult,
};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use uuid::Uuid;

const SNAPSHOT_SCAN_PAGE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const SNAPSHOT_STREAM_TIMEOUT: Duration = Duration::from_secs(2 * 60);
const SNAPSHOT_CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const INSTALLED_PRODUCTION_APP_PATH: &str = "/Applications/TelevyBackup.app";
const SNAPSHOT_ACCESS_MANIFEST: &str = "snapshot-access/service.json";

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
    #[error("snapshot access request timed out during {operation}")]
    Timeout { operation: &'static str },
    #[error("snapshot access JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub struct SnapshotClient {
    socket_path: PathBuf,
    config_root: Option<PathBuf>,
    expected_access_app_path: Option<PathBuf>,
    require_registered_service: bool,
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
            config_root: None,
            expected_access_app_path: None,
            require_registered_service: false,
        }
    }
}

impl SnapshotClient {
    pub fn for_environment(config_root: &Path, data_root: &Path) -> Self {
        let app_path = current_app_path();
        let expected_access_app_path = app_path
            .as_ref()
            .map(|app| app.join(ACCESS_BUNDLE_RELATIVE_PATH));
        let require_registered_service = app_path.as_deref()
            == Some(Path::new(INSTALLED_PRODUCTION_APP_PATH))
            && std::env::var("TELEVYBACKUP_DISABLE_KEYCHAIN").as_deref() != Ok("1")
            && config_root == default_production_directory("TelevyBackup")
            && data_root == default_production_directory("TelevyBackup");
        Self {
            socket_path: data_root.join("snapshot-access/access.sock"),
            config_root: Some(config_root.to_path_buf()),
            expected_access_app_path,
            require_registered_service,
        }
    }

    #[allow(dead_code)]
    pub fn with_socket_path(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
            config_root: None,
            expected_access_app_path: None,
            require_registered_service: false,
        }
    }

    #[allow(dead_code)]
    pub async fn ensure_running(&self) -> Result<(), SnapshotClientError> {
        self.status().await.map(|_| ())
    }

    pub async fn status(&self) -> Result<StatusResult, SnapshotClientError> {
        let response = self.request(Method::Status).await?;
        match response.result {
            Some(ResponseResult::Status(result)) => {
                validate_component_status(&result)?;
                self.validate_runtime_identity(&result)?;
                Ok(result)
            }
            _ => Err(SnapshotClientError::Protocol(
                "status response missing result".into(),
            )),
        }
    }

    pub async fn probe_volume(&self, target_id: &str) -> Result<ProbeResult, SnapshotClientError> {
        self.ensure_compatible().await?;
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
        self.ensure_compatible().await?;
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

    async fn ensure_compatible(&self) -> Result<(), SnapshotClientError> {
        self.status().await.map(|_| ())
    }

    fn validate_runtime_identity(&self, status: &StatusResult) -> Result<(), SnapshotClientError> {
        if let Some(expected) = &self.expected_access_app_path {
            let actual = status.access_app_path.as_deref().map(Path::new);
            if actual != Some(expected.as_path()) {
                return Err(SnapshotClientError::Protocol(
                    "Snapshot Access socket is not owned by the embedded helper".into(),
                ));
            }
        }
        if self.require_registered_service {
            let config_root = self.config_root.as_deref().ok_or_else(|| {
                SnapshotClientError::Protocol(
                    "Snapshot Access registration root is unavailable".into(),
                )
            })?;
            let manifest_path = config_root.join(SNAPSHOT_ACCESS_MANIFEST);
            let bytes = fs::read(&manifest_path).map_err(|error| {
                SnapshotClientError::Protocol(format!(
                    "Snapshot Access registration is not committed: {error}"
                ))
            })?;
            let manifest: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| {
                SnapshotClientError::Protocol(format!(
                    "Snapshot Access registration manifest is invalid: {error}"
                ))
            })?;
            let required = [
                ("managedBy", "smappservice"),
                ("migrationState", "ready"),
                ("bundleId", ACCESS_BUNDLE_ID),
                ("relativeAppPath", ACCESS_BUNDLE_RELATIVE_PATH),
                ("componentVersion", COMPONENT_VERSION),
            ];
            if required.iter().any(|(field, expected)| {
                manifest.get(field).and_then(serde_json::Value::as_str) != Some(*expected)
            }) || manifest
                .get("protocolVersion")
                .and_then(serde_json::Value::as_u64)
                != Some(u64::from(PROTOCOL_VERSION))
                || manifest.get("appPath").and_then(serde_json::Value::as_str)
                    != self
                        .expected_access_app_path
                        .as_deref()
                        .and_then(Path::to_str)
            {
                return Err(SnapshotClientError::Protocol(
                    "Snapshot Access registration is not ready for the embedded helper".into(),
                ));
            }
        }
        Ok(())
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
        let timeout = request_timeout(&method);
        self.request_with_timeout(method, timeout).await
    }

    async fn request_with_timeout(
        &self,
        method: Method,
        timeout: Duration,
    ) -> Result<Response, SnapshotClientError> {
        let request = Request {
            version: PROTOCOL_VERSION,
            request_id: Uuid::new_v4().to_string(),
            method,
        };
        let mut stream = tokio::time::timeout(timeout, UnixStream::connect(&self.socket_path))
            .await
            .map_err(|_| SnapshotClientError::Timeout {
                operation: "connect",
            })?
            .map_err(|error| SnapshotClientError::Unavailable(error.to_string()))?;
        let encoded = serde_json::to_vec(&request)?;
        tokio::time::timeout(timeout, stream.write_all(&encoded))
            .await
            .map_err(|_| SnapshotClientError::Timeout { operation: "write" })??;
        tokio::time::timeout(timeout, stream.write_all(b"\n"))
            .await
            .map_err(|_| SnapshotClientError::Timeout { operation: "write" })??;
        tokio::time::timeout(timeout, stream.flush())
            .await
            .map_err(|_| SnapshotClientError::Timeout { operation: "flush" })??;
        let mut line = String::new();
        tokio::time::timeout(timeout, BufReader::new(stream).read_line(&mut line))
            .await
            .map_err(|_| SnapshotClientError::Timeout { operation: "read" })??;
        decode_response(line.trim(), &request.request_id)
    }

    fn request_blocking(&self, method: Method) -> Result<Response, SnapshotClientError> {
        let timeout = request_timeout(&method);
        let request = Request {
            version: PROTOCOL_VERSION,
            request_id: Uuid::new_v4().to_string(),
            method,
        };
        let mut stream = std::os::unix::net::UnixStream::connect(&self.socket_path)
            .map_err(|error| SnapshotClientError::Unavailable(error.to_string()))?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let mut encoded = serde_json::to_vec(&request)?;
        encoded.push(b'\n');
        stream.write_all(&encoded)?;
        let mut line = String::new();
        std::io::BufReader::new(stream).read_line(&mut line)?;
        decode_response(line.trim(), &request.request_id)
    }
}

fn current_app_path() -> Option<PathBuf> {
    std::env::current_exe().ok()?.ancestors().find_map(|path| {
        let name = path.file_name()?;
        if name == "TelevyBackup.app" || name == "TelevyBackup Dev.app" {
            Some(path.to_path_buf())
        } else {
            None
        }
    })
}

fn default_production_directory(name: &str) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/Application Support")
        .join(name)
}

fn validate_component_status(status: &StatusResult) -> Result<(), SnapshotClientError> {
    if status.access_app_version != COMPONENT_VERSION {
        return Err(SnapshotClientError::Protocol(format!(
            "incompatible Snapshot Access component version: {}",
            status.access_app_version
        )));
    }
    Ok(())
}

fn request_timeout(method: &Method) -> Duration {
    match method {
        Method::ScanPage { .. } => SNAPSHOT_SCAN_PAGE_TIMEOUT,
        Method::OpenReadStream { .. }
        | Method::ReadStream { .. }
        | Method::CloseReadStream { .. } => SNAPSHOT_STREAM_TIMEOUT,
        Method::Status
        | Method::ProbeVolume { .. }
        | Method::AcquireLease { .. }
        | Method::ReleaseLease { .. } => SNAPSHOT_CONTROL_TIMEOUT,
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
            .map_err(|error| snapshot_access_error("scan", error))
    }

    fn open(&self, relative_path: &str) -> CoreResult<Box<dyn Read + Send>> {
        self.client
            .open_read_stream_blocking(&self.lease_id, relative_path)
            .map(|stream| Box::new(stream) as Box<dyn Read + Send>)
            .map_err(|error| snapshot_access_error("read", error))
    }

    fn is_brokered(&self) -> bool {
        true
    }
}

fn snapshot_access_error(operation: &str, error: SnapshotClientError) -> CoreError {
    CoreError::SnapshotAccess {
        message: format!("snapshot access {operation} failed: {error}"),
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

fn decode_response(line: &str, request_id: &str) -> Result<Response, SnapshotClientError> {
    let response: Response = serde_json::from_str(line)?;
    if response.version != PROTOCOL_VERSION {
        return Err(SnapshotClientError::Protocol(
            "unsupported access app response version".into(),
        ));
    }
    if response.request_id != request_id {
        return Err(SnapshotClientError::Protocol(
            "snapshot access response request id does not match".into(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brokered_source_reports_snapshot_access_failures_without_invalidating_config() {
        let temp = tempfile::tempdir().unwrap();
        let source = BrokeredSnapshotSource::new(
            SnapshotClient::with_socket_path(temp.path().join("missing.sock")),
            "lease-1",
            temp.path().join("logical-source"),
        );

        let error = source.entries(None).unwrap_err();

        assert_eq!(error.code(), "snapshot.access_failed");
    }

    #[test]
    fn scan_page_allows_a_full_snapshot_inventory_walk() {
        assert_eq!(
            request_timeout(&Method::ScanPage {
                lease_id: "lease-1".into(),
                cursor: None,
                limit: 512,
            }),
            SNAPSHOT_SCAN_PAGE_TIMEOUT
        );
    }

    #[tokio::test]
    async fn async_request_times_out_when_helper_does_not_reply() {
        let temp = tempfile::tempdir().unwrap();
        let socket = temp.path().join("access.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let (_stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
        });

        let client = SnapshotClient::with_socket_path(socket);
        let error = client
            .request_with_timeout(Method::Status, Duration::from_millis(20))
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            SnapshotClientError::Timeout { operation: "read" }
        ));
        server.await.unwrap();
    }

    #[test]
    fn response_validation_requires_the_request_id() {
        let response = Response::ok("request-1", ResponseResult::Status(StatusResult::default()));
        let encoded = serde_json::to_string(&response).unwrap();
        assert!(decode_response(&encoded, "request-1").is_ok());
        assert!(matches!(
            decode_response(&encoded, "request-2"),
            Err(SnapshotClientError::Protocol(message))
                if message.contains("request id")
        ));
    }

    #[test]
    fn status_rejects_an_incompatible_component_version() {
        let status = StatusResult {
            access_app_version: "0.1.0".into(),
            ..Default::default()
        };
        assert!(matches!(
            validate_component_status(&status),
            Err(SnapshotClientError::Protocol(message))
                if message.contains("incompatible Snapshot Access component version")
        ));
    }

    #[test]
    fn runtime_identity_rejects_a_helper_outside_the_product_bundle() {
        let client = SnapshotClient {
            socket_path: PathBuf::from("/tmp/access.sock"),
            config_root: None,
            expected_access_app_path: Some(PathBuf::from(
                "/Applications/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app",
            )),
            require_registered_service: false,
        };
        let status = StatusResult {
            access_app_path: Some("/Users/test/Projects/TelevyBackup Snapshot Access.app".into()),
            ..Default::default()
        };

        assert!(matches!(
            client.validate_runtime_identity(&status),
            Err(SnapshotClientError::Protocol(message))
                if message.contains("not owned by the embedded helper")
        ));
    }

    #[test]
    fn runtime_identity_requires_a_committed_embedded_registration() {
        let temp = tempfile::tempdir().unwrap();
        let app_path = PathBuf::from("/Applications/TelevyBackup.app");
        let helper_path = app_path.join(ACCESS_BUNDLE_RELATIVE_PATH);
        let manifest_path = temp.path().join(SNAPSHOT_ACCESS_MANIFEST);
        fs::create_dir_all(manifest_path.parent().unwrap()).unwrap();
        fs::write(
            &manifest_path,
            serde_json::json!({
                "managedBy": "smappservice",
                "migrationState": "ready",
                "bundleId": ACCESS_BUNDLE_ID,
                "relativeAppPath": ACCESS_BUNDLE_RELATIVE_PATH,
                "componentVersion": COMPONENT_VERSION,
                "protocolVersion": PROTOCOL_VERSION,
                "appPath": helper_path,
            })
            .to_string(),
        )
        .unwrap();
        let client = SnapshotClient {
            socket_path: PathBuf::from("/tmp/access.sock"),
            config_root: Some(temp.path().to_path_buf()),
            expected_access_app_path: Some(helper_path.clone()),
            require_registered_service: true,
        };
        let status = StatusResult {
            access_app_path: Some(helper_path.to_string_lossy().into_owned()),
            ..Default::default()
        };

        assert!(client.validate_runtime_identity(&status).is_ok());

        fs::write(
            &manifest_path,
            serde_json::json!({
                "managedBy": "smappservice",
                "migrationState": "pending",
                "bundleId": ACCESS_BUNDLE_ID,
                "relativeAppPath": ACCESS_BUNDLE_RELATIVE_PATH,
                "componentVersion": COMPONENT_VERSION,
                "protocolVersion": PROTOCOL_VERSION,
                "appPath": helper_path,
            })
            .to_string(),
        )
        .unwrap();
        assert!(matches!(
            client.validate_runtime_identity(&status),
            Err(SnapshotClientError::Protocol(message))
                if message.contains("registration is not ready")
        ));
    }
}
