use std::fs;
use std::io::{BufRead, Write};
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use plist::Value;
use serde::Serialize;
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream as TokioUnixStream};
use uuid::Uuid;

use crate::{
    DEFAULT_MOUNT_HELPER_JOURNAL, DEFAULT_MOUNT_HELPER_SOCKET, MOUNT_HELPER_PROTOCOL_VERSION,
    MountLeaseResult, MountMethod, MountReleaseResult, MountRequest, MountResponse,
    MountResponseResult, MountSnapshotRef, MountStatusResult, validate_mount_request,
};

pub const ROOT_MOUNT_HELPER_VERSION: &str = "0.1.0";
const SOCKET_DIRECTORY_MODE: u32 = 0o711;
const STATUS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MUTATING_REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Error)]
pub enum MountHelperError {
    #[error("mount helper unavailable: {0}")]
    Unavailable(String),
    #[error("mount helper rejected request ({code}): {message}")]
    Rejected { code: String, message: String },
    #[error("mount helper protocol error: {0}")]
    Protocol(String),
    #[error("mount helper I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{program} failed ({status}): {stderr}")]
    Command {
        program: String,
        status: String,
        stderr: String,
    },
    #[error("mount helper JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mount helper database error: {0}")]
    Sqlx(#[from] sqlx::Error),
}

#[derive(Debug, Clone, Serialize)]
struct JournalSnapshot {
    device_identifier: String,
    uuid: String,
}

pub fn configured_socket_path() -> PathBuf {
    std::env::var_os("TELEVYBACKUP_SNAPSHOT_MOUNT_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_MOUNT_HELPER_SOCKET))
}

pub fn configured_journal_path() -> PathBuf {
    std::env::var_os("TELEVYBACKUP_SNAPSHOT_MOUNT_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_MOUNT_HELPER_JOURNAL))
}

pub fn status() -> Result<MountStatusResult, MountHelperError> {
    let response = request(MountMethod::Status)?;
    match response.result {
        Some(MountResponseResult::Status(result)) => {
            validate_status_result(&result)?;
            Ok(result)
        }
        _ => Err(MountHelperError::Protocol(
            "status response missing result".into(),
        )),
    }
}

fn validate_status_result(result: &MountStatusResult) -> Result<(), MountHelperError> {
    if result.helper_version != ROOT_MOUNT_HELPER_VERSION {
        return Err(MountHelperError::Protocol(format!(
            "incompatible mount helper version: {}",
            result.helper_version
        )));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn mount(
    uid: u32,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    snapshot_uuid: &str,
    snapshot_name: &str,
    source_mount_point: &Path,
    mount_root: &Path,
    snapshot_manifest: &[MountSnapshotRef],
) -> Result<MountLeaseResult, MountHelperError> {
    let response = request(MountMethod::Mount {
        uid,
        lease_id: lease_id.to_string(),
        volume_uuid: volume_uuid.to_string(),
        device_identifier: device_identifier.to_string(),
        snapshot_uuid: snapshot_uuid.to_string(),
        snapshot_name: snapshot_name.to_string(),
        source_mount_point: source_mount_point.to_string_lossy().into_owned(),
        mount_root: mount_root.to_string_lossy().into_owned(),
        snapshot_manifest: snapshot_manifest.to_vec(),
    })?;
    match response.result {
        Some(MountResponseResult::Mounted(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "mount response missing result".into(),
        )),
    }
}

pub fn release(uid: u32, lease_id: &str) -> Result<MountReleaseResult, MountHelperError> {
    let response = request(MountMethod::Release {
        uid,
        lease_id: lease_id.to_string(),
    })?;
    match response.result {
        Some(MountResponseResult::Released(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "release response missing result".into(),
        )),
    }
}

pub fn cleanup(
    uid: u32,
    cleanup_id: &str,
    snapshot_manifest: &[MountSnapshotRef],
) -> Result<MountReleaseResult, MountHelperError> {
    let response = request(MountMethod::Cleanup {
        uid,
        cleanup_id: cleanup_id.to_string(),
        snapshot_manifest: snapshot_manifest.to_vec(),
    })?;
    match response.result {
        Some(MountResponseResult::Cleaned(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "cleanup response missing result".into(),
        )),
    }
}

fn request(method: MountMethod) -> Result<MountResponse, MountHelperError> {
    let timeout = request_timeout(&method);
    let mut stream = UnixStream::connect(configured_socket_path())
        .map_err(|error| MountHelperError::Unavailable(error.to_string()))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let request = MountRequest {
        version: MOUNT_HELPER_PROTOCOL_VERSION,
        request_id: Uuid::new_v4().to_string(),
        method,
    };
    let mut encoded = serde_json::to_vec(&request)?;
    encoded.push(b'\n');
    stream.write_all(&encoded)?;
    stream.flush()?;
    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line)?;
    let response: MountResponse = serde_json::from_str(line.trim())?;
    validate_response(&response, &request.request_id)?;
    if !response.ok {
        return Err(MountHelperError::Rejected {
            code: response.code.unwrap_or_else(|| "rejected".into()),
            message: response
                .message
                .unwrap_or_else(|| "mount helper rejected request".into()),
        });
    }
    Ok(response)
}

fn validate_response(response: &MountResponse, request_id: &str) -> Result<(), MountHelperError> {
    if response.version != MOUNT_HELPER_PROTOCOL_VERSION {
        return Err(MountHelperError::Protocol(
            "unsupported mount helper response version".into(),
        ));
    }
    if response.request_id != request_id {
        return Err(MountHelperError::Protocol(
            "mount helper response request id does not match".into(),
        ));
    }
    Ok(())
}

fn request_timeout(method: &MountMethod) -> Duration {
    match method {
        MountMethod::Status => STATUS_REQUEST_TIMEOUT,
        MountMethod::Mount { .. } | MountMethod::Release { .. } | MountMethod::Cleanup { .. } => {
            MUTATING_REQUEST_TIMEOUT
        }
    }
}

pub async fn run_server() -> Result<(), MountHelperError> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(MountHelperError::Protocol(
            "snapshot mount helper must run as root".into(),
        ));
    }
    let journal_path = configured_journal_path();
    let journal_parent = journal_path
        .parent()
        .ok_or_else(|| MountHelperError::Protocol("journal has no parent".into()))?;
    ensure_root_directory(journal_parent)?;
    // Mount requests are serialized by the journal. A single connection keeps
    // launchd startup deterministic and avoids parallel SQLite initialization.
    let journal = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(
            SqliteConnectOptions::new()
                .filename(&journal_path)
                .create_if_missing(true),
        )
        .await?;
    ensure_root_file(&journal_path)?;
    init_journal(&journal).await?;
    recover_journal(&journal).await?;

    let socket_path = configured_socket_path();
    let socket_parent = socket_path
        .parent()
        .ok_or_else(|| MountHelperError::Protocol("socket has no parent".into()))?;
    ensure_root_socket_directory(socket_parent)?;
    if socket_path.exists() {
        let metadata = fs::symlink_metadata(&socket_path)?;
        if metadata.uid() != 0 || !metadata.file_type().is_socket() {
            return Err(MountHelperError::Protocol(
                "unsafe mount helper socket".into(),
            ));
        }
        fs::remove_file(&socket_path)?;
    }
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666))?;
    loop {
        let (stream, _) = listener.accept().await?;
        let journal = journal.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, journal).await {
                eprintln!("snapshot mount helper connection failed: {error}");
            }
        });
    }
}

async fn init_journal(pool: &SqlitePool) -> Result<(), MountHelperError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS mounts (
            lease_id TEXT PRIMARY KEY,
            uid INTEGER NOT NULL,
            volume_uuid TEXT NOT NULL,
            device_identifier TEXT NOT NULL,
            snapshot_name TEXT NOT NULL,
            mount_root TEXT NOT NULL,
            snapshot_manifest TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    Ok(())
}

async fn recover_journal(pool: &SqlitePool) -> Result<(), MountHelperError> {
    let rows = sqlx::query(
        "SELECT lease_id, mount_root, snapshot_manifest FROM mounts WHERE state != 'released'",
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        let lease_id: String = row.try_get("lease_id")?;
        let mount_root: String = row.try_get("mount_root")?;
        let manifest_json: String = row.try_get("snapshot_manifest")?;
        let manifest =
            serde_json::from_str::<Vec<MountSnapshotRef>>(&manifest_json).unwrap_or_default();
        let cleanup_state = if mount_root.is_empty() {
            if delete_manifest(&manifest) {
                "released"
            } else {
                "cleanup_pending"
            }
        } else {
            cleanup_mount(Path::new(&mount_root), &manifest)
        };
        update_state(pool, &lease_id, cleanup_state).await?;
    }
    Ok(())
}

async fn serve_connection(
    stream: TokioUnixStream,
    journal: SqlitePool,
) -> Result<(), MountHelperError> {
    let peer_uid = peer_uid(stream.as_raw_fd())?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    while reader.read_line(&mut line).await? > 0 {
        let request: Result<MountRequest, _> = serde_json::from_str(line.trim());
        let response = match request {
            Ok(request) => handle_request(request, peer_uid, &journal).await,
            Err(error) => MountResponse::error("unknown", "invalid_request", error.to_string()),
        };
        if !response.ok {
            eprintln!(
                "snapshot mount helper request failed request_id={} code={} message={}",
                response.request_id,
                response.code.as_deref().unwrap_or("unknown"),
                response.message.as_deref().unwrap_or("unknown")
            );
        }
        write_half
            .write_all(serde_json::to_string(&response)?.as_bytes())
            .await?;
        write_half.write_all(b"\n").await?;
        write_half.flush().await?;
        line.clear();
    }
    Ok(())
}

async fn handle_request(
    request: MountRequest,
    peer_uid: u32,
    journal: &SqlitePool,
) -> MountResponse {
    let request_id = request.request_id.clone();
    if let Err(error) = validate_mount_request(&request) {
        return MountResponse::error(request_id, "invalid_request", error);
    }
    let result = match request.method {
        MountMethod::Status => status_result(journal)
            .await
            .map(MountResponseResult::Status),
        MountMethod::Mount {
            uid,
            lease_id,
            volume_uuid,
            device_identifier,
            snapshot_uuid,
            snapshot_name,
            source_mount_point,
            mount_root,
            snapshot_manifest,
        } => mount_request(
            peer_uid,
            uid,
            &lease_id,
            &volume_uuid,
            &device_identifier,
            &snapshot_uuid,
            &snapshot_name,
            Path::new(&source_mount_point),
            Path::new(&mount_root),
            &snapshot_manifest,
            journal,
        )
        .await
        .map(MountResponseResult::Mounted),
        MountMethod::Release { uid, lease_id } => {
            release_request(peer_uid, uid, &lease_id, journal)
                .await
                .map(MountResponseResult::Released)
        }
        MountMethod::Cleanup {
            uid,
            cleanup_id,
            snapshot_manifest,
        } => cleanup_request(peer_uid, uid, &cleanup_id, &snapshot_manifest, journal)
            .await
            .map(MountResponseResult::Cleaned),
    };
    match result {
        Ok(result) => MountResponse::ok(request_id, result),
        Err(error) => MountResponse::error(request_id, error_code(&error), error.to_string()),
    }
}

fn error_code(error: &MountHelperError) -> &'static str {
    match error {
        MountHelperError::Rejected { .. } => "rejected",
        MountHelperError::Unavailable(_) => "unavailable",
        MountHelperError::Protocol(_) => "invalid_request",
        MountHelperError::Command { .. } => "command_failed",
        MountHelperError::Io(_) => "io",
        MountHelperError::Json(_) => "protocol",
        MountHelperError::Sqlx(_) => "journal",
    }
}

async fn status_result(pool: &SqlitePool) -> Result<MountStatusResult, MountHelperError> {
    let active_mounts = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mounts WHERE state IN ('prepared', 'active', 'cleanup_pending')",
    )
    .fetch_one(pool)
    .await? as u32;
    Ok(MountStatusResult {
        helper_version: ROOT_MOUNT_HELPER_VERSION.to_string(),
        active_mounts,
    })
}

#[allow(clippy::too_many_arguments)]
async fn mount_request(
    peer_uid: u32,
    uid: u32,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    snapshot_uuid: &str,
    snapshot_name: &str,
    source_mount_point: &Path,
    mount_root: &Path,
    manifest: &[MountSnapshotRef],
    journal: &SqlitePool,
) -> Result<MountLeaseResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "mount request UID does not match peer".into(),
        ));
    }
    validate_lease_id(lease_id)?;
    validate_uuid(volume_uuid)?;
    validate_device(device_identifier)?;
    validate_uuid(snapshot_uuid)?;
    validate_snapshot_name(snapshot_name)?;
    if manifest.is_empty()
        || !manifest.iter().any(|snapshot| {
            snapshot.device_identifier == device_identifier && snapshot.uuid == snapshot_uuid
        })
    {
        return Err(MountHelperError::Protocol(
            "snapshot manifest does not identify the requested volume".into(),
        ));
    }
    for snapshot in manifest {
        validate_device(&snapshot.device_identifier)?;
        validate_uuid(&snapshot.uuid)?;
    }
    validate_mount_root(mount_root, uid)?;
    validate_source_mount(source_mount_point, volume_uuid, device_identifier)?;
    let existing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mounts WHERE lease_id = ? AND state != 'released'",
    )
    .bind(lease_id)
    .fetch_one(journal)
    .await?;
    if existing != 0 {
        return Err(MountHelperError::Protocol("lease already exists".into()));
    }
    let manifest_json = serde_json::to_string(
        &manifest
            .iter()
            .map(|snapshot| JournalSnapshot {
                device_identifier: snapshot.device_identifier.clone(),
                uuid: snapshot.uuid.clone(),
            })
            .collect::<Vec<_>>(),
    )?;
    sqlx::query(
        "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'prepared', datetime('now'))",
    )
    .bind(lease_id)
    .bind(i64::from(uid))
    .bind(volume_uuid)
    .bind(device_identifier)
    .bind(snapshot_name)
    .bind(mount_root.to_string_lossy().as_ref())
    .bind(&manifest_json)
    .execute(journal)
    .await?;

    let mount_result = run_command(
        "/sbin/mount_apfs",
        [
            "-s",
            snapshot_name,
            source_mount_point.to_string_lossy().as_ref(),
            mount_root.to_string_lossy().as_ref(),
        ],
    );
    if let Err(error) = mount_result {
        let cleanup_state = if delete_manifest(manifest) {
            "released"
        } else {
            "cleanup_pending"
        };
        update_state(journal, lease_id, cleanup_state).await?;
        return Err(error);
    }
    sqlx::query("UPDATE mounts SET state = 'active' WHERE lease_id = ?")
        .bind(lease_id)
        .execute(journal)
        .await?;
    Ok(MountLeaseResult {
        lease_id: lease_id.to_string(),
        mount_root: mount_root.to_string_lossy().into_owned(),
    })
}

async fn release_request(
    peer_uid: u32,
    uid: u32,
    lease_id: &str,
    journal: &SqlitePool,
) -> Result<MountReleaseResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "release request UID does not match peer".into(),
        ));
    }
    validate_lease_id(lease_id)?;
    let row = sqlx::query(
        "SELECT uid, mount_root, snapshot_manifest FROM mounts WHERE lease_id = ? AND state != 'released'",
    )
    .bind(lease_id)
    .fetch_optional(journal)
    .await?
    .ok_or_else(|| MountHelperError::Protocol("mount lease not found".into()))?;
    let stored_uid: i64 = row.try_get("uid")?;
    if stored_uid != i64::from(uid) {
        return Err(MountHelperError::Protocol(
            "mount lease belongs to another user".into(),
        ));
    }
    let mount_root: String = row.try_get("mount_root")?;
    let manifest_json: String = row.try_get("snapshot_manifest")?;
    let manifest =
        serde_json::from_str::<Vec<MountSnapshotRef>>(&manifest_json).unwrap_or_default();
    let cleanup_state = cleanup_mount(Path::new(&mount_root), &manifest);
    update_state(journal, lease_id, cleanup_state).await?;
    Ok(MountReleaseResult {
        lease_id: lease_id.to_string(),
        cleanup_state: cleanup_state.to_string(),
    })
}

async fn cleanup_request(
    peer_uid: u32,
    uid: u32,
    cleanup_id: &str,
    manifest: &[MountSnapshotRef],
    journal: &SqlitePool,
) -> Result<MountReleaseResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "cleanup request UID does not match peer".into(),
        ));
    }
    validate_lease_id(cleanup_id)?;
    if manifest.is_empty() {
        return Err(MountHelperError::Protocol(
            "cleanup manifest is empty".into(),
        ));
    }
    for snapshot in manifest {
        validate_device(&snapshot.device_identifier)?;
        validate_uuid(&snapshot.uuid)?;
    }
    let manifest_json = serde_json::to_string(manifest)?;
    let existing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mounts WHERE lease_id = ? AND state != 'released'",
    )
    .bind(cleanup_id)
    .fetch_one(journal)
    .await?;
    if existing != 0 {
        return Err(MountHelperError::Protocol(
            "cleanup id already has an active journal entry".into(),
        ));
    }
    sqlx::query(
        "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at) VALUES (?, ?, '', '', '', '', ?, 'prepared', datetime('now'))",
    )
    .bind(cleanup_id)
    .bind(i64::from(uid))
    .bind(&manifest_json)
    .execute(journal)
    .await?;
    let cleanup_state = if delete_manifest(manifest) {
        "released"
    } else {
        "cleanup_pending"
    };
    update_state(journal, cleanup_id, cleanup_state).await?;
    Ok(MountReleaseResult {
        lease_id: cleanup_id.to_string(),
        cleanup_state: cleanup_state.to_string(),
    })
}

fn cleanup_mount(mount_root: &Path, manifest: &[MountSnapshotRef]) -> &'static str {
    let unmount_ok = match mounted_snapshot_path(mount_root) {
        Ok(true) => run_command(
            "/sbin/umount",
            ["-f", mount_root.to_string_lossy().as_ref()],
        )
        .is_ok(),
        // A prepared journal row can survive a crash before mount_apfs completes. In that case
        // there is no mount to remove, so UUID-scoped snapshot cleanup can proceed.
        Ok(false) => true,
        Err(_) => false,
    };
    let delete_ok = delete_manifest(manifest);
    if unmount_ok && delete_ok {
        "released"
    } else {
        "cleanup_pending"
    }
}

fn mounted_snapshot_path(path: &Path) -> Result<bool, MountHelperError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(MountHelperError::Io(error)),
    };
    let parent = path
        .parent()
        .ok_or_else(|| MountHelperError::Protocol("mount root has no parent".into()))?;
    let parent_metadata = match fs::metadata(parent) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(MountHelperError::Io(error)),
    };
    Ok(metadata.dev() != parent_metadata.dev())
}

fn delete_manifest(manifest: &[MountSnapshotRef]) -> bool {
    manifest.iter().all(|snapshot| {
        run_command(
            "/usr/sbin/diskutil",
            [
                "apfs",
                "deleteSnapshot",
                &snapshot.device_identifier,
                "-uuid",
                &snapshot.uuid,
            ],
        )
        .is_ok()
    })
}

async fn update_state(
    journal: &SqlitePool,
    lease_id: &str,
    state: &str,
) -> Result<(), MountHelperError> {
    sqlx::query("UPDATE mounts SET state = ? WHERE lease_id = ?")
        .bind(state)
        .bind(lease_id)
        .execute(journal)
        .await?;
    Ok(())
}

fn validate_mount_root(path: &Path, uid: u32) -> Result<(), MountHelperError> {
    if !path.is_absolute() {
        return Err(MountHelperError::Protocol(
            "mount root must be absolute".into(),
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o022 != 0 {
        return Err(MountHelperError::Protocol(
            "mount root must be an existing private directory owned by the caller".into(),
        ));
    }
    let mut current = path;
    while let Some(parent) = current.parent() {
        let metadata = fs::symlink_metadata(current)?;
        if metadata.file_type().is_symlink() || metadata.mode() & 0o022 != 0 {
            return Err(MountHelperError::Protocol(
                "mount root path contains a symlink or world-writable directory".into(),
            ));
        }
        if parent == current {
            break;
        }
        current = parent;
    }
    Ok(())
}

fn validate_source_mount(
    source_mount_point: &Path,
    expected_volume_uuid: &str,
    expected_device: &str,
) -> Result<(), MountHelperError> {
    if !source_mount_point.is_absolute() || !source_mount_point.is_dir() {
        return Err(MountHelperError::Protocol(
            "source mount point is not a directory".into(),
        ));
    }
    let output = run_command_output(
        "/usr/sbin/diskutil",
        [
            "info",
            "-plist",
            source_mount_point.to_string_lossy().as_ref(),
        ],
    )?;
    let value = Value::from_reader_xml(output.as_slice())
        .map_err(|error| MountHelperError::Protocol(format!("invalid volume plist: {error}")))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| MountHelperError::Protocol("volume plist is not a dictionary".into()))?;
    let filesystem = dict
        .get("FilesystemName")
        .and_then(Value::as_string)
        .unwrap_or_default();
    let volume_uuid = dict
        .get("APFSVolumeUUID")
        .and_then(Value::as_string)
        .or_else(|| dict.get("VolumeUUID").and_then(Value::as_string))
        .unwrap_or_default();
    let device = dict
        .get("DeviceIdentifier")
        .and_then(Value::as_string)
        .unwrap_or_default();
    if !filesystem.eq_ignore_ascii_case("APFS")
        || !volume_uuid.eq_ignore_ascii_case(expected_volume_uuid)
        || device != expected_device
    {
        return Err(MountHelperError::Protocol(
            "source mount identity does not match the lease".into(),
        ));
    }
    Ok(())
}

fn validate_lease_id(value: &str) -> Result<(), MountHelperError> {
    if value.len() > 128
        || value.is_empty()
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() && byte != b'-')
    {
        return Err(MountHelperError::Protocol("invalid mount lease id".into()));
    }
    Ok(())
}

fn validate_uuid(value: &str) -> Result<(), MountHelperError> {
    if value.len() != 36
        || value.bytes().filter(|byte| *byte == b'-').count() != 4
        || value
            .bytes()
            .any(|byte| !byte.is_ascii_hexdigit() && byte != b'-')
    {
        return Err(MountHelperError::Protocol(
            "invalid snapshot or volume UUID".into(),
        ));
    }
    Ok(())
}

fn validate_device(value: &str) -> Result<(), MountHelperError> {
    if value.is_empty()
        || value.len() > 64
        || value.bytes().any(|byte| !byte.is_ascii_alphanumeric())
    {
        return Err(MountHelperError::Protocol(
            "invalid APFS device identifier".into(),
        ));
    }
    Ok(())
}

fn validate_snapshot_name(value: &str) -> Result<(), MountHelperError> {
    if value.is_empty() || value.len() > 256 || value.bytes().any(|byte| byte.is_ascii_control()) {
        return Err(MountHelperError::Protocol("invalid snapshot name".into()));
    }
    Ok(())
}

fn ensure_root_directory(path: &Path) -> Result<(), MountHelperError> {
    if !path.exists() {
        fs::create_dir_all(path)?;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(MountHelperError::Protocol(format!(
            "unsafe root-owned mount helper directory: {}",
            path.display()
        )));
    }
    Ok(())
}

// The daemon is root-owned, but the peer-UID-checked IPC socket must be reachable from each
// logged-in user's session. Execute-only access permits socket traversal without directory listing.
fn ensure_root_socket_directory(path: &Path) -> Result<(), MountHelperError> {
    if !path.exists() {
        fs::create_dir_all(path)?;
    }
    fs::set_permissions(path, fs::Permissions::from_mode(SOCKET_DIRECTORY_MODE))?;
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(MountHelperError::Protocol(format!(
            "unsafe root-owned mount helper socket directory: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_root_file(path: &Path) -> Result<(), MountHelperError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_file() || metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
        return Err(MountHelperError::Protocol(format!(
            "unsafe root-owned mount helper journal: {}",
            path.display()
        )));
    }
    Ok(())
}

fn run_command<const N: usize, S: AsRef<std::ffi::OsStr>>(
    program: &str,
    args: [S; N],
) -> Result<(), MountHelperError> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(MountHelperError::Command {
        program: program.to_string(),
        status: output.status.to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn run_command_output<const N: usize, S: AsRef<std::ffi::OsStr>>(
    program: &str,
    args: [S; N],
) -> Result<Vec<u8>, MountHelperError> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(MountHelperError::Command {
        program: program.to_string(),
        status: output.status.to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn peer_uid(fd: RawFd) -> Result<u32, MountHelperError> {
    #[cfg(target_os = "macos")]
    {
        let mut euid = 0_u32;
        let mut egid = 0_u32;
        let result = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
        if result != 0 {
            return Err(MountHelperError::Io(std::io::Error::last_os_error()));
        }
        Ok(euid)
    }
    #[cfg(target_os = "linux")]
    {
        let mut credentials = unsafe { std::mem::zeroed::<libc::ucred>() };
        let mut length = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let result = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                (&mut credentials as *mut libc::ucred).cast(),
                &mut length,
            )
        };
        if result != 0 {
            return Err(MountHelperError::Io(std::io::Error::last_os_error()));
        }
        Ok(credentials.uid)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = fd;
        Err(MountHelperError::Protocol(
            "peer UID lookup is unsupported on this platform".into(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MountMethod, MountRequest};

    #[test]
    fn mount_request_rejects_arbitrary_device_and_path_inputs() {
        assert!(validate_lease_id("A8A38394-49AE-42F9-85FB-14DD9EA549FA").is_ok());
        assert!(validate_lease_id("../escape").is_err());
        assert!(validate_device("disk5s1").is_ok());
        assert!(validate_device("/dev/disk5s1").is_err());
        assert!(validate_uuid("A8A38394-49AE-42F9-85FB-14DD9EA549FA").is_ok());
        assert!(validate_uuid("not-a-uuid").is_err());
    }

    #[test]
    fn mount_protocol_round_trips_manifest_and_selected_uuid() {
        let request = MountRequest {
            version: MOUNT_HELPER_PROTOCOL_VERSION,
            request_id: "r1".into(),
            method: MountMethod::Mount {
                uid: 501,
                lease_id: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
                volume_uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
                device_identifier: "disk5s1".into(),
                snapshot_uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
                snapshot_name: "com.apple.TimeMachine.2026-09-08-010203.local".into(),
                source_mount_point: "/System/Volumes/Data".into(),
                mount_root: "/Users/test/Library/Application Support/TelevyBackup/snapshot-access/mounts/501/A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
                snapshot_manifest: vec![MountSnapshotRef {
                    device_identifier: "disk5s1".into(),
                    uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
                }],
            },
        };
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: MountRequest = serde_json::from_str(&encoded).unwrap();
        assert!(validate_mount_request(&decoded).is_ok());
        assert!(matches!(decoded.method, MountMethod::Mount { .. }));
    }

    #[test]
    fn status_uses_a_short_timeout_but_mutations_allow_snapshot_cleanup() {
        assert_eq!(
            request_timeout(&MountMethod::Status),
            STATUS_REQUEST_TIMEOUT
        );
        assert_eq!(
            request_timeout(&MountMethod::Release {
                uid: 501,
                lease_id: "lease".into(),
            }),
            MUTATING_REQUEST_TIMEOUT
        );
    }

    #[test]
    fn unmounted_private_directory_is_safe_to_recover() {
        let temp = tempfile::tempdir().unwrap();
        let mount_root = temp.path().join("mount");
        fs::create_dir(&mount_root).unwrap();
        assert!(!mounted_snapshot_path(&mount_root).unwrap());
    }

    #[test]
    fn mount_root_rejects_group_writable_ancestors() {
        let temp = tempfile::tempdir().unwrap();
        let unsafe_parent = temp.path().join("group-writable");
        let mount_root = unsafe_parent.join("mount");
        fs::create_dir(&unsafe_parent).unwrap();
        fs::create_dir(&mount_root).unwrap();
        fs::set_permissions(&unsafe_parent, fs::Permissions::from_mode(0o770)).unwrap();
        let uid = unsafe { libc::geteuid() };
        assert!(validate_mount_root(&mount_root, uid).is_err());
    }

    #[test]
    fn socket_directory_allows_traversal_but_not_unprivileged_writes() {
        assert_ne!(SOCKET_DIRECTORY_MODE & 0o001, 0);
        assert_eq!(SOCKET_DIRECTORY_MODE & 0o022, 0);
        assert_eq!(SOCKET_DIRECTORY_MODE & 0o044, 0);
    }

    #[test]
    fn mount_response_validation_requires_protocol_and_request_identity() {
        let response = MountResponse::ok(
            "request-1",
            MountResponseResult::Status(MountStatusResult::default()),
        );
        assert!(validate_response(&response, "request-1").is_ok());

        let mut wrong_version = response.clone();
        wrong_version.version += 1;
        assert!(validate_response(&wrong_version, "request-1").is_err());

        let mut wrong_request = response;
        wrong_request.request_id = "request-2".into();
        assert!(validate_response(&wrong_request, "request-1").is_err());
    }

    #[test]
    fn mount_status_rejects_an_incompatible_component_version() {
        let result = MountStatusResult {
            helper_version: "0.0.0".into(),
            active_mounts: 0,
        };
        assert!(validate_status_result(&result).is_err());
    }
}
