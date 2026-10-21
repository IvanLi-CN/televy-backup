use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use plist::Value;
use sqlx::{
    Row, SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use thiserror::Error;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream as TokioUnixStream};
use tokio::sync::Mutex as AsyncMutex;
use uuid::Uuid;

use crate::{
    DEFAULT_MOUNT_HELPER_JOURNAL, DEFAULT_MOUNT_HELPER_MUTATION_LOCK, DEFAULT_MOUNT_HELPER_SOCKET,
    MOUNT_HELPER_PROTOCOL_VERSION, MountLeaseResult, MountMethod, MountRecoveryLease,
    MountReleaseResult, MountRequest, MountResponse, MountResponseResult, MountSnapshotRef,
    MountStatusResult, validate_mount_request,
};

pub const ROOT_MOUNT_HELPER_VERSION: &str = "0.5.0";
pub const LEGACY_ROOT_MOUNT_HELPER_VERSION: &str = "0.9.8";
const SOCKET_DIRECTORY_MODE: u32 = 0o711;
const MOUNT_ROOT_BASE: &str = "/private/var/tmp/televybackup-snapshot-mounts";
const STATUS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const MUTATING_REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const RECOVERY_AGE_SECONDS: &str = "-300 seconds";
const AUTHORIZE_SOURCE_PATH_ARG: &str = "--authorize-source-path";

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
    #[error("stage={stage} command={command} exit={status} channel={channel} summary={summary}")]
    Command {
        stage: &'static str,
        command: &'static str,
        status: String,
        channel: &'static str,
        summary: String,
    },
    #[error("mount helper JSON error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("mount helper database error: {0}")]
    Sqlx(#[from] sqlx::Error),
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

pub fn configured_mutation_lock_path() -> PathBuf {
    std::env::var_os("TELEVYBACKUP_SNAPSHOT_MOUNT_LOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            let journal = configured_journal_path();
            if journal == Path::new(DEFAULT_MOUNT_HELPER_JOURNAL) {
                PathBuf::from(DEFAULT_MOUNT_HELPER_MUTATION_LOCK)
            } else {
                journal.with_file_name("mutation.lock")
            }
        })
}

pub struct MutationFileLock {
    file: fs::File,
}

impl Drop for MutationFileLock {
    fn drop(&mut self) {
        // Unlock explicitly so the guard's cross-process lifetime remains obvious.
        unsafe {
            libc::flock(self.file.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

pub fn acquire_mutation_lock() -> Result<MutationFileLock, MountHelperError> {
    let path = configured_mutation_lock_path();
    let parent = path
        .parent()
        .ok_or_else(|| MountHelperError::Protocol("mutation lock has no parent".into()))?;
    ensure_root_directory(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(MountHelperError::Protocol(
            "unsafe root-owned snapshot mount mutation lock".into(),
        ));
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    ensure_root_owned_file(&path, "snapshot mount mutation lock")?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
        return Err(MountHelperError::Io(std::io::Error::last_os_error()));
    }
    Ok(MutationFileLock { file })
}

fn configured_mount_root_base() -> PathBuf {
    PathBuf::from(MOUNT_ROOT_BASE)
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

pub fn status_for_volume(
    uid: u32,
    volume_uuid: &str,
) -> Result<MountStatusResult, MountHelperError> {
    let response = request(MountMethod::StatusVolume {
        uid,
        volume_uuid: volume_uuid.to_string(),
    })?;
    match response.result {
        Some(MountResponseResult::Status(result)) => {
            validate_status_result(&result)?;
            Ok(result)
        }
        _ => Err(MountHelperError::Protocol(
            "volume status response missing result".into(),
        )),
    }
}

pub fn reserve(
    uid: u32,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    source_path: &Path,
) -> Result<MountReleaseResult, MountHelperError> {
    let response = request(MountMethod::Reserve {
        uid,
        lease_id: lease_id.to_string(),
        volume_uuid: volume_uuid.to_string(),
        device_identifier: device_identifier.to_string(),
        source_path: source_path.to_string_lossy().into_owned(),
    })?;
    match response.result {
        Some(MountResponseResult::Reserved(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "reserve response missing result".into(),
        )),
    }
}

pub fn renew(uid: u32, lease_id: &str) -> Result<MountReleaseResult, MountHelperError> {
    let response = request(MountMethod::Renew {
        uid,
        lease_id: lease_id.to_string(),
    })?;
    match response.result {
        Some(MountResponseResult::Renewed(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "renew response missing result".into(),
        )),
    }
}

fn validate_status_result(result: &MountStatusResult) -> Result<(), MountHelperError> {
    if result.helper_version != ROOT_MOUNT_HELPER_VERSION
        && result.helper_version != LEGACY_ROOT_MOUNT_HELPER_VERSION
    {
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

pub fn record_cleanup(uid: u32, lease_id: &str) -> Result<MountReleaseResult, MountHelperError> {
    let response = request(MountMethod::RecordCleanup {
        uid,
        lease_id: lease_id.to_string(),
    })?;
    match response.result {
        Some(MountResponseResult::CleanupRecorded(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "cleanup record response missing result".into(),
        )),
    }
}

pub fn cleanup(uid: u32, cleanup_id: &str) -> Result<MountReleaseResult, MountHelperError> {
    let response = request(MountMethod::Cleanup {
        uid,
        cleanup_id: cleanup_id.to_string(),
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
        MountMethod::Status | MountMethod::StatusVolume { .. } => STATUS_REQUEST_TIMEOUT,
        MountMethod::Reserve { .. }
        | MountMethod::Renew { .. }
        | MountMethod::Mount { .. }
        | MountMethod::Release { .. }
        | MountMethod::RecordCleanup { .. }
        | MountMethod::Cleanup { .. } => MUTATING_REQUEST_TIMEOUT,
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
    ensure_mount_root_base(&configured_mount_root_base())?;
    // Keep one SQLite connection and serialize requests so lease existence checks, mount-root
    // creation, and journal transitions cannot race across IPC connections.
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
    let startup_mutation_lock = acquire_mutation_lock()?;
    recover_journal(&journal).await?;
    drop(startup_mutation_lock);

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
    let mutation_lock = Arc::new(AsyncMutex::new(()));
    loop {
        let (stream, _) = listener.accept().await?;
        let journal = journal.clone();
        let mutation_lock = Arc::clone(&mutation_lock);
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, journal, mutation_lock).await {
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
            source_path TEXT NOT NULL DEFAULT '',
            snapshot_name TEXT NOT NULL,
            mount_root TEXT NOT NULL,
            mount_device INTEGER NOT NULL DEFAULT 0,
            snapshot_manifest TEXT NOT NULL,
            reservation_manifest TEXT NOT NULL DEFAULT '[]',
            reservation_manifest_valid INTEGER NOT NULL DEFAULT 0,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL,
            last_renewed_at TEXT NOT NULL DEFAULT ''
        )",
    )
    .execute(pool)
    .await?;
    let _ = sqlx::query("ALTER TABLE mounts ADD COLUMN mount_device INTEGER NOT NULL DEFAULT 0")
        .execute(pool)
        .await;
    let _ = sqlx::query(
        "ALTER TABLE mounts ADD COLUMN reservation_manifest TEXT NOT NULL DEFAULT '[]'",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query(
        "ALTER TABLE mounts ADD COLUMN reservation_manifest_valid INTEGER NOT NULL DEFAULT 0",
    )
    .execute(pool)
    .await;
    let _ = sqlx::query("ALTER TABLE mounts ADD COLUMN source_path TEXT NOT NULL DEFAULT ''")
        .execute(pool)
        .await;
    let _ = sqlx::query("ALTER TABLE mounts ADD COLUMN last_renewed_at TEXT NOT NULL DEFAULT ''")
        .execute(pool)
        .await;
    let _ =
        sqlx::query("UPDATE mounts SET last_renewed_at = created_at WHERE last_renewed_at = ''")
            .execute(pool)
            .await;
    Ok(())
}

async fn recover_journal(pool: &SqlitePool) -> Result<(), MountHelperError> {
    let rows = sqlx::query(
        "SELECT lease_id, uid, volume_uuid, device_identifier, mount_root, mount_device, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state FROM mounts WHERE state != 'released'",
    )
    .fetch_all(pool)
    .await?;
    for row in rows {
        let lease_id: String = row.try_get("lease_id")?;
        let uid: i64 = row.try_get("uid").unwrap_or_default();
        let volume_uuid: String = row.try_get("volume_uuid")?;
        let device_identifier: String = row.try_get("device_identifier")?;
        let mount_root: String = row.try_get("mount_root")?;
        let mount_device: i64 = row.try_get("mount_device").unwrap_or_default();
        let manifest_json: String = row.try_get("snapshot_manifest")?;
        let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
        let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
        let state: String = row.try_get("state")?;
        if state == "reserved" && mount_root.is_empty() && manifest_json == "[]" {
            // Snapshot creation may have completed while the access process was disconnected.
            // Keep the reservation blocked until a later owner can reconcile the post-reservation
            // inventory; releasing it here would orphan an unrecorded snapshot.
            continue;
        }
        if reservation_manifest_valid != 1 {
            let _ = update_state(pool, &lease_id, "cleanup_pending").await;
            continue;
        }
        let manifest = parse_journal_manifest(&manifest_json).ok();
        if let Some(ref manifest) = manifest
            && !inventory_matches_expected(&device_identifier, &reservation_manifest_json, manifest)
        {
            let _ = update_state(pool, &lease_id, "cleanup_pending").await;
            continue;
        }
        let expected_device = u64::try_from(mount_device)
            .ok()
            .filter(|device| *device != 0);
        let cleanup_state = match manifest {
            None => "cleanup_pending",
            Some(manifest)
                if mount_root.is_empty()
                    && validate_device_volume_binding(&device_identifier, &volume_uuid).is_ok()
                    && manifest.iter().all(|snapshot| {
                        snapshot
                            .device_identifier
                            .eq_ignore_ascii_case(&device_identifier)
                    }) =>
            {
                delete_manifest_and_record_tombstone(
                    pool,
                    &lease_id,
                    &volume_uuid,
                    &device_identifier,
                    &reservation_manifest_json,
                    &manifest_json,
                    &manifest,
                )
                .await
            }
            Some(manifest) => match expected_mount_root(uid, &lease_id) {
                Ok(expected_root)
                    if Path::new(&mount_root) == expected_root
                        && validate_managed_mount_root(&expected_root)
                        && validate_device_volume_binding(&device_identifier, &volume_uuid)
                            .is_ok()
                        && manifest.iter().all(|snapshot| {
                            snapshot
                                .device_identifier
                                .eq_ignore_ascii_case(&device_identifier)
                        }) =>
                {
                    // A crash can leave a prepared or active row after mount_apfs succeeded but
                    // before the mounted device identity was persisted. The managed root is
                    // root-owned and cannot be replaced by the requesting user, so cleanup can
                    // inspect and unmount it.
                    cleanup_mount(
                        &expected_root,
                        &manifest,
                        expected_device,
                        &volume_uuid,
                        pool,
                        &lease_id,
                        &device_identifier,
                        &reservation_manifest_json,
                        &manifest_json,
                    )
                    .await
                }
                // Rows from the pre-root-owned layout or malformed journals cannot be safely
                // mapped to a stable target. Keep the volume blocked instead of resolving a
                // user-controlled path.
                _ => "cleanup_pending",
            },
        };
        update_state(pool, &lease_id, cleanup_state).await?;
    }
    Ok(())
}

fn expected_mount_root(uid: i64, lease_id: &str) -> Result<PathBuf, MountHelperError> {
    let uid = u32::try_from(uid)
        .map_err(|_| MountHelperError::Protocol("invalid journal mount owner".into()))?;
    validate_lease_id(lease_id)?;
    Ok(configured_mount_root_base()
        .join(uid.to_string())
        .join(lease_id))
}

fn create_mount_root(uid: u32, lease_id: &str) -> Result<PathBuf, MountHelperError> {
    let base = configured_mount_root_base();
    ensure_mount_root_base(&base)?;
    let uid_root = base.join(uid.to_string());
    match fs::symlink_metadata(&uid_root) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(&uid_root)?;
            fs::set_permissions(&uid_root, fs::Permissions::from_mode(0o711))?;
        }
        Err(error) => return Err(MountHelperError::Io(error)),
    }
    ensure_root_owned_directory(&uid_root, 0o711)?;
    let mount_root = uid_root.join(lease_id);
    fs::create_dir(&mount_root)?;
    fs::set_permissions(&mount_root, fs::Permissions::from_mode(0o755))?;
    ensure_root_owned_directory(&mount_root, 0o755)?;
    Ok(mount_root)
}

fn ensure_mount_root_base(path: &Path) -> Result<(), MountHelperError> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o711))?;
        }
        Err(error) => return Err(MountHelperError::Io(error)),
    }
    ensure_root_owned_directory(path, 0o711)
}

fn validate_managed_mount_root(path: &Path) -> bool {
    let base = configured_mount_root_base();
    let Some(uid_root) = path.parent() else {
        return false;
    };
    let base_ok = fs::symlink_metadata(&base)
        .map(|metadata| {
            metadata.is_dir()
                && metadata.uid() == 0
                && metadata.mode() & 0o022 == 0
                && !metadata.file_type().is_symlink()
        })
        .unwrap_or(false);
    let uid_root_ok = fs::symlink_metadata(uid_root)
        .map(|metadata| {
            metadata.is_dir()
                && metadata.uid() == 0
                && metadata.mode() & 0o022 == 0
                && !metadata.file_type().is_symlink()
        })
        .unwrap_or(false);
    let mount_root_ok = fs::symlink_metadata(path)
        .map(|metadata| {
            metadata.is_dir()
                && metadata.uid() == 0
                && metadata.mode() & 0o022 == 0
                && !metadata.file_type().is_symlink()
        })
        .unwrap_or_else(|error| error.kind() == std::io::ErrorKind::NotFound);
    path.starts_with(&base) && base_ok && uid_root_ok && mount_root_ok
}

fn ensure_root_owned_directory(path: &Path, expected_mode: u32) -> Result<(), MountHelperError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.is_dir()
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
        || metadata.mode() & 0o777 != expected_mode
    {
        return Err(MountHelperError::Protocol(
            "unsafe root-owned snapshot mount directory".into(),
        ));
    }
    Ok(())
}

async fn serve_connection(
    stream: TokioUnixStream,
    journal: SqlitePool,
    mutation_lock: Arc<AsyncMutex<()>>,
) -> Result<(), MountHelperError> {
    let peer_uid = peer_uid(stream.as_raw_fd())?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();
    while reader.read_line(&mut line).await? > 0 {
        let request: Result<MountRequest, _> = serde_json::from_str(line.trim());
        let response = match request {
            Ok(request) => {
                let needs_file_lock = matches!(
                    &request.method,
                    MountMethod::Reserve { .. }
                        | MountMethod::Renew { .. }
                        | MountMethod::Mount { .. }
                        | MountMethod::Release { .. }
                        | MountMethod::RecordCleanup { .. }
                        | MountMethod::Cleanup { .. }
                );
                if needs_file_lock {
                    let _mutation_guard = mutation_lock.lock().await;
                    match tokio::task::spawn_blocking(acquire_mutation_lock).await {
                        Ok(Ok(_file_lock)) => handle_request(request, peer_uid, &journal).await,
                        Ok(Err(error)) => MountResponse::error(
                            request.request_id,
                            "mutation_lock_unavailable",
                            error.to_string(),
                        ),
                        Err(error) => MountResponse::error(
                            request.request_id,
                            "mutation_lock_unavailable",
                            format!("mutation lock task failed: {error}"),
                        ),
                    }
                } else {
                    handle_request(request, peer_uid, &journal).await
                }
            }
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
        MountMethod::Status => status_result(journal, peer_uid)
            .await
            .map(MountResponseResult::Status),
        MountMethod::StatusVolume { uid, volume_uuid } => {
            status_for_volume_result(peer_uid, uid, &volume_uuid, journal)
                .await
                .map(MountResponseResult::Status)
        }
        MountMethod::Reserve {
            uid,
            lease_id,
            volume_uuid,
            device_identifier,
            source_path,
        } => reserve_request(
            peer_uid,
            uid,
            &lease_id,
            &volume_uuid,
            &device_identifier,
            Path::new(&source_path),
            journal,
        )
        .await
        .map(MountResponseResult::Reserved),
        MountMethod::Renew { uid, lease_id } => renew_request(peer_uid, uid, &lease_id, journal)
            .await
            .map(MountResponseResult::Renewed),
        MountMethod::Mount {
            uid,
            lease_id,
            volume_uuid,
            device_identifier,
            snapshot_uuid,
            snapshot_name,
            source_mount_point,
            mount_root: _requested_mount_root,
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
        MountMethod::RecordCleanup { uid, lease_id } => {
            record_cleanup_request(peer_uid, uid, &lease_id, journal)
                .await
                .map(MountResponseResult::CleanupRecorded)
        }
        MountMethod::Cleanup { uid, cleanup_id } => {
            cleanup_request(peer_uid, uid, &cleanup_id, journal)
                .await
                .map(MountResponseResult::Cleaned)
        }
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

async fn status_result(
    pool: &SqlitePool,
    peer_uid: u32,
) -> Result<MountStatusResult, MountHelperError> {
    let active_mounts = (if peer_uid == 0 {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM mounts WHERE state IN ('reserved', 'prepared', 'active', 'cleanup_pending')",
        )
        .fetch_one(pool)
        .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM mounts WHERE uid = ? AND state IN ('reserved', 'prepared', 'active', 'cleanup_pending')",
        )
        .bind(i64::from(peer_uid))
        .fetch_one(pool)
        .await?
    }) as u32;
    let pending_cleanup = (if peer_uid == 0 {
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM mounts WHERE state = 'cleanup_pending'")
            .fetch_one(pool)
            .await?
    } else {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM mounts WHERE uid = ? AND state = 'cleanup_pending'",
        )
        .bind(i64::from(peer_uid))
        .fetch_one(pool)
        .await?
    }) as u32;
    let blocked_volume_uuids = if peer_uid == 0 {
        sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT volume_uuid FROM mounts WHERE state IN ('reserved', 'prepared', 'active', 'cleanup_pending') AND volume_uuid != ''",
        )
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_scalar::<_, String>(
            "SELECT DISTINCT volume_uuid FROM mounts WHERE uid = ? AND state IN ('reserved', 'prepared', 'active', 'cleanup_pending') AND volume_uuid != ''",
        )
        .bind(i64::from(peer_uid))
        .fetch_all(pool)
        .await?
    };
    Ok(MountStatusResult {
        helper_version: ROOT_MOUNT_HELPER_VERSION.to_string(),
        build_commit: option_env!("TELEVYBACKUP_BUILD_COMMIT").map(str::to_owned),
        active_mounts,
        pending_cleanup,
        blocked_volume_uuids,
        recovery_leases: Vec::new(),
    })
}

async fn status_for_volume_result(
    peer_uid: u32,
    uid: u32,
    volume_uuid: &str,
    pool: &SqlitePool,
) -> Result<MountStatusResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "volume status request UID does not match peer".into(),
        ));
    }
    validate_uuid(volume_uuid)?;
    let active_mounts = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mounts WHERE lower(volume_uuid) = lower(?) AND state IN ('reserved', 'prepared', 'active', 'cleanup_pending')",
    )
    .bind(volume_uuid)
    .fetch_one(pool)
    .await? as u32;
    let pending_cleanup = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM mounts WHERE lower(volume_uuid) = lower(?) AND state = 'cleanup_pending'",
    )
    .bind(volume_uuid)
    .fetch_one(pool)
    .await? as u32;
    let blocked_volume_uuids = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT volume_uuid FROM mounts WHERE lower(volume_uuid) = lower(?) AND state IN ('reserved', 'prepared', 'active', 'cleanup_pending') AND volume_uuid != ''",
    )
    .bind(volume_uuid)
    .fetch_all(pool)
    .await?;
    let recovery_rows = sqlx::query(
        "SELECT lease_id, state FROM mounts WHERE lower(volume_uuid) = lower(?) AND last_renewed_at <= datetime('now', ?) AND ((state IN ('reserved', 'cleanup_pending') AND snapshot_manifest = '[]' AND mount_root = '') OR (uid = ? AND state IN ('prepared', 'cleanup_pending') AND mount_root = ''))",
    )
    .bind(volume_uuid)
    .bind(RECOVERY_AGE_SECONDS)
    .bind(i64::from(uid))
    .fetch_all(pool)
    .await?;
    let recovery_leases = recovery_rows
        .into_iter()
        .map(|row| {
            Ok(MountRecoveryLease {
                lease_id: row.try_get("lease_id")?,
                state: row.try_get("state")?,
            })
        })
        .collect::<Result<Vec<_>, sqlx::Error>>()?;
    Ok(MountStatusResult {
        helper_version: ROOT_MOUNT_HELPER_VERSION.to_string(),
        build_commit: option_env!("TELEVYBACKUP_BUILD_COMMIT").map(str::to_owned),
        active_mounts,
        pending_cleanup,
        blocked_volume_uuids,
        recovery_leases,
    })
}

async fn audit_released_tombstones(
    journal: &SqlitePool,
    volume_uuid: &str,
    device_identifier: &str,
) -> Result<(), MountHelperError> {
    let rows = sqlx::query(
        "SELECT lease_id, device_identifier, reservation_manifest FROM mounts WHERE lower(volume_uuid) = lower(?) AND state = 'released' AND reservation_manifest_valid = 1",
    )
    .bind(volume_uuid)
    .fetch_all(journal)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let current_manifest = snapshot_inventory_manifest(device_identifier)?;
    for row in rows {
        let lease_id: String = row.try_get("lease_id")?;
        let stored_device: String = row.try_get("device_identifier")?;
        let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
        let baseline = parse_reservation_manifest(&reservation_manifest_json);
        let matches = stored_device.eq_ignore_ascii_case(device_identifier)
            && baseline
                .as_ref()
                .is_ok_and(|baseline| manifests_equal(&current_manifest, baseline));
        if !matches {
            sqlx::query("UPDATE mounts SET state = 'cleanup_pending' WHERE lease_id = ? AND state = 'released'")
                .bind(lease_id)
                .execute(journal)
                .await?;
            return Err(MountHelperError::Protocol(
                "released snapshot tombstone inventory changed".into(),
            ));
        }
    }
    Ok(())
}

async fn reserve_request(
    peer_uid: u32,
    uid: u32,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    source_path: &Path,
    journal: &SqlitePool,
) -> Result<MountReleaseResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "reserve request UID does not match peer".into(),
        ));
    }
    validate_lease_id(lease_id)?;
    validate_uuid(volume_uuid)?;
    validate_device(device_identifier)?;
    validate_device_volume_binding(device_identifier, volume_uuid)?;
    let source = validate_source_path(source_path, volume_uuid, device_identifier)?;
    authorize_source_path(uid, source_path, &source)?;
    source.recheck(source_path)?;
    audit_released_tombstones(journal, volume_uuid, device_identifier).await?;
    let blocked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mounts WHERE lower(volume_uuid) = lower(?) AND state IN ('reserved', 'prepared', 'active', 'cleanup_pending')",
    )
    .bind(volume_uuid)
    .fetch_one(journal)
    .await?;
    if blocked > 0 {
        return Err(MountHelperError::Protocol(
            "source volume has an active mount or pending snapshot cleanup".into(),
        ));
    }
    let reservation_manifest = snapshot_inventory_manifest(device_identifier)?;
    let reservation_manifest_json = serde_json::to_string(&reservation_manifest)?;
    source.recheck(source_path)?;
    sqlx::query(
        "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, source_path, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?, ?, ?, ?, ?, '', '', '[]', ?, 1, 'reserved', datetime('now'), datetime('now'))",
    )
    .bind(lease_id)
    .bind(i64::from(uid))
    .bind(volume_uuid)
    .bind(device_identifier)
    .bind(source_path.to_string_lossy().as_ref())
    .bind(&reservation_manifest_json)
    .execute(journal)
    .await?;
    if let Err(error) = source.recheck(source_path) {
        update_state(journal, lease_id, "cleanup_pending").await?;
        return Err(error);
    }
    Ok(MountReleaseResult {
        lease_id: lease_id.to_string(),
        cleanup_state: "reserved".to_string(),
    })
}

async fn renew_request(
    peer_uid: u32,
    uid: u32,
    lease_id: &str,
    journal: &SqlitePool,
) -> Result<MountReleaseResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "renew request UID does not match peer".into(),
        ));
    }
    validate_lease_id(lease_id)?;
    let state: Option<String> = sqlx::query_scalar(
        "SELECT state FROM mounts WHERE lease_id = ? AND uid = ? AND mount_root = ''",
    )
    .bind(lease_id)
    .bind(i64::from(uid))
    .fetch_optional(journal)
    .await?;
    if state.as_deref() == Some("prepared") {
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: "prepared".to_string(),
        });
    }
    let updated = sqlx::query(
        "UPDATE mounts SET last_renewed_at = datetime('now') WHERE lease_id = ? AND uid = ? AND state = 'reserved' AND mount_root = '' AND snapshot_manifest = '[]'",
    )
    .bind(lease_id)
    .bind(i64::from(uid))
    .execute(journal)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(MountHelperError::Protocol(
            "snapshot reservation is no longer renewable".into(),
        ));
    }
    Ok(MountReleaseResult {
        lease_id: lease_id.to_string(),
        cleanup_state: "reserved".to_string(),
    })
}

async fn record_cleanup_request(
    peer_uid: u32,
    uid: u32,
    lease_id: &str,
    journal: &SqlitePool,
) -> Result<MountReleaseResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "cleanup record UID does not match peer".into(),
        ));
    }
    validate_lease_id(lease_id)?;
    let row = sqlx::query(
        "SELECT uid, volume_uuid, device_identifier, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state FROM mounts WHERE lease_id = ?",
    )
    .bind(lease_id)
    .fetch_optional(journal)
    .await?
    .ok_or_else(|| MountHelperError::Protocol("cleanup reservation not found".into()))?;
    let stored_uid: i64 = row.try_get("uid")?;
    if stored_uid != i64::from(uid) {
        return Err(MountHelperError::Protocol(
            "cleanup reservation belongs to another user".into(),
        ));
    }
    let mount_root: String = row.try_get("mount_root")?;
    let state: String = row.try_get("state")?;
    let stored_manifest: String = row.try_get("snapshot_manifest")?;
    let volume_uuid: String = row.try_get("volume_uuid")?;
    let device_identifier: String = row.try_get("device_identifier")?;
    let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
    let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
    if reservation_manifest_valid != 1 {
        return Err(MountHelperError::Protocol(
            "cleanup reservation has no auditable baseline".into(),
        ));
    }
    if state == "prepared" && !mount_root.is_empty() && stored_manifest != "[]" {
        return Err(MountHelperError::Protocol(
            "cleanup reservation is already mounted".into(),
        ));
    }
    if state == "prepared" && mount_root.is_empty() && stored_manifest != "[]" {
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: "prepared".to_string(),
        });
    }
    if state != "reserved" || !mount_root.is_empty() || stored_manifest != "[]" {
        return Err(MountHelperError::Protocol(
            "cleanup reservation is no longer available".into(),
        ));
    }
    if validate_device_volume_binding(&device_identifier, &volume_uuid).is_err() {
        return Err(MountHelperError::Protocol(
            "cleanup reservation device no longer matches the journaled volume".into(),
        ));
    }
    let reservation_manifest = parse_reservation_manifest(&reservation_manifest_json)?;
    // Snapshot creation is performed inside the root helper's serialized mutation section. This
    // prevents a caller from turning another process's snapshot into its deletion manifest.
    let before_manifest = snapshot_inventory_manifest(&device_identifier)?;
    if !manifests_equal(&before_manifest, &reservation_manifest) {
        return Err(MountHelperError::Protocol(
            "cleanup reservation baseline changed before recording".into(),
        ));
    }
    run_command(
        "snapshot_create",
        "tmutil.localsnapshot",
        "/usr/bin/tmutil",
        ["localsnapshot"],
    )?;
    let current_manifest = snapshot_inventory_manifest(&device_identifier)?;
    let manifest: Vec<_> = current_manifest
        .iter()
        .filter(|snapshot| {
            !reservation_manifest.iter().any(|reserved| {
                reserved
                    .device_identifier
                    .eq_ignore_ascii_case(&snapshot.device_identifier)
                    && reserved.uuid.eq_ignore_ascii_case(&snapshot.uuid)
            })
        })
        .cloned()
        .collect();
    if manifest.is_empty() {
        return Err(MountHelperError::Protocol(
            "cleanup reservation has no snapshot created after the reservation".into(),
        ));
    }
    if manifest.len() != 1 {
        let _ = update_state(journal, lease_id, "cleanup_pending").await;
        return Err(MountHelperError::Protocol(
            "cleanup reservation has an ambiguous snapshot creation result".into(),
        ));
    }
    let confirmed_manifest = snapshot_inventory_manifest(&device_identifier)?;
    if !manifests_equal(&confirmed_manifest, &current_manifest) {
        let _ = update_state(journal, lease_id, "cleanup_pending").await;
        return Err(MountHelperError::Protocol(
            "snapshot inventory changed while recording cleanup".into(),
        ));
    }
    let manifest_json = serde_json::to_string(&manifest)?;
    let updated = sqlx::query(
        "UPDATE mounts SET snapshot_manifest = ?, state = 'prepared' WHERE lease_id = ? AND state = 'reserved' AND snapshot_manifest = '[]' AND reservation_manifest = ? AND reservation_manifest_valid = 1",
    )
    .bind(&manifest_json)
    .bind(lease_id)
    .bind(&reservation_manifest_json)
    .execute(journal)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(MountHelperError::Protocol(
            "cleanup reservation was claimed by another request".into(),
        ));
    }
    let final_manifest = snapshot_inventory_manifest(&device_identifier)?;
    let mut expected_manifest = reservation_manifest;
    expected_manifest.extend(manifest.iter().cloned());
    if !manifests_equal(&final_manifest, &expected_manifest) {
        let _ = update_state(journal, lease_id, "cleanup_pending").await;
        return Err(MountHelperError::Protocol(
            "snapshot inventory changed after cleanup was recorded".into(),
        ));
    }
    Ok(MountReleaseResult {
        lease_id: lease_id.to_string(),
        cleanup_state: "prepared".to_string(),
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
    validate_source_mount(source_mount_point, volume_uuid, device_identifier)?;
    validate_snapshot_binding(device_identifier, snapshot_uuid, snapshot_name)?;
    let blocked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM mounts WHERE lower(volume_uuid) = lower(?) AND lease_id != ? AND state IN ('reserved', 'prepared', 'active', 'cleanup_pending')",
    )
    .bind(volume_uuid)
    .bind(lease_id)
    .fetch_one(journal)
    .await?;
    if blocked > 0 {
        return Err(MountHelperError::Protocol(
            "source volume has an active mount or pending snapshot cleanup".into(),
        ));
    }
    let mount_root = expected_mount_root(i64::from(uid), lease_id)?;
    let existing = sqlx::query(
        "SELECT uid, volume_uuid, device_identifier, source_path, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state FROM mounts WHERE lease_id = ?",
    )
    .bind(lease_id)
    .fetch_optional(journal)
    .await?;
    let row = existing.ok_or_else(|| {
        MountHelperError::Protocol("mount requires a recorded snapshot reservation".into())
    })?;
    let stored_uid: i64 = row.try_get("uid")?;
    let stored_volume_uuid: String = row.try_get("volume_uuid")?;
    let stored_device_identifier: String = row.try_get("device_identifier")?;
    let stored_source_path: String = row.try_get("source_path").unwrap_or_default();
    let stored_mount_root: String = row.try_get("mount_root")?;
    let stored_manifest_json: String = row.try_get("snapshot_manifest")?;
    let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
    let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
    let stored_manifest = parse_journal_manifest(&stored_manifest_json)?;
    let state: String = row.try_get("state")?;
    if stored_uid != i64::from(uid)
        || !stored_volume_uuid.eq_ignore_ascii_case(volume_uuid)
        || stored_device_identifier != device_identifier
        || !stored_mount_root.is_empty()
        || state != "prepared"
        || reservation_manifest_valid != 1
        || !manifests_equal(&stored_manifest, manifest)
    {
        return Err(MountHelperError::Protocol(
            "mount request does not match the recorded snapshot reservation".into(),
        ));
    }
    let stored_source_path = Path::new(&stored_source_path);
    let source = validate_source_path(
        stored_source_path,
        &stored_volume_uuid,
        &stored_device_identifier,
    )?;
    authorize_source_path(uid, stored_source_path, &source)?;
    source.recheck(stored_source_path)?;
    let journal_update = sqlx::query(
        "UPDATE mounts SET snapshot_name = ?, mount_root = ? WHERE lease_id = ? AND state = 'prepared' AND mount_root = ''",
    )
    .bind(snapshot_name)
    .bind(mount_root.to_string_lossy().as_ref())
    .bind(lease_id)
    .execute(journal)
    .await?;
    if journal_update.rows_affected() != 1 {
        return Err(MountHelperError::Protocol(
            "mount reservation was claimed by another request".into(),
        ));
    }

    if let Err(error) = create_mount_root(uid, lease_id) {
        // The prepared row is the recovery record for the snapshot. If the target already exists,
        // ownership is ambiguous, so leave the row pending instead of unmounting or deleting
        // another attempt's resources.
        let cleanup_state = match fs::symlink_metadata(&mount_root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if delete_manifest_and_record_tombstone(
                    journal,
                    lease_id,
                    volume_uuid,
                    device_identifier,
                    &reservation_manifest_json,
                    &stored_manifest_json,
                    manifest,
                )
                .await
                    == "released"
                {
                    "released"
                } else {
                    "cleanup_pending"
                }
            }
            _ => "cleanup_pending",
        };
        update_state(journal, lease_id, cleanup_state).await?;
        return Err(error);
    }

    // Re-check the source identity immediately before the privileged mount command. The first
    // validation protects the request contract; this second check closes the journal/update
    // window in which a system mount could have changed.
    let source_device =
        match validate_source_mount(source_mount_point, volume_uuid, device_identifier) {
            Ok(device) => device,
            Err(error) => {
                let cleanup_state = cleanup_mount(
                    &mount_root,
                    manifest,
                    None,
                    volume_uuid,
                    journal,
                    lease_id,
                    device_identifier,
                    &reservation_manifest_json,
                    &stored_manifest_json,
                )
                .await;
                update_state(journal, lease_id, cleanup_state).await?;
                return Err(error);
            }
        };
    let source_device = source_device.to_string_lossy().into_owned();
    let mount_result = run_command(
        "snapshot_mount",
        "mount_apfs",
        "/sbin/mount_apfs",
        [
            "-s",
            snapshot_name,
            source_device.as_str(),
            mount_root.to_string_lossy().as_ref(),
        ],
    );
    if let Err(error) = mount_result {
        let cleanup_state = cleanup_mount(
            &mount_root,
            manifest,
            None,
            volume_uuid,
            journal,
            lease_id,
            device_identifier,
            &reservation_manifest_json,
            &stored_manifest_json,
        )
        .await;
        update_state(journal, lease_id, cleanup_state).await?;
        return Err(error);
    }
    match mounted_snapshot_path(&mount_root) {
        Ok(true) => {}
        Ok(false) => {
            let cleanup_state = cleanup_mount(
                &mount_root,
                manifest,
                None,
                volume_uuid,
                journal,
                lease_id,
                device_identifier,
                &reservation_manifest_json,
                &stored_manifest_json,
            )
            .await;
            update_state(journal, lease_id, cleanup_state).await?;
            return Err(MountHelperError::Protocol(
                "mount_apfs did not expose the requested snapshot mount".into(),
            ));
        }
        Err(error) => {
            let cleanup_state = cleanup_mount(
                &mount_root,
                manifest,
                None,
                volume_uuid,
                journal,
                lease_id,
                device_identifier,
                &reservation_manifest_json,
                &stored_manifest_json,
            )
            .await;
            update_state(journal, lease_id, cleanup_state).await?;
            return Err(error);
        }
    }
    let mount_device = match fs::metadata(&mount_root) {
        Ok(metadata) => metadata.dev(),
        Err(error) => {
            let cleanup_state = cleanup_mount(
                &mount_root,
                manifest,
                None,
                volume_uuid,
                journal,
                lease_id,
                device_identifier,
                &reservation_manifest_json,
                &stored_manifest_json,
            )
            .await;
            update_state(journal, lease_id, cleanup_state).await?;
            return Err(MountHelperError::Io(error));
        }
    };
    if mount_device == 0 {
        let cleanup_state = cleanup_mount(
            &mount_root,
            manifest,
            None,
            volume_uuid,
            journal,
            lease_id,
            device_identifier,
            &reservation_manifest_json,
            &stored_manifest_json,
        )
        .await;
        update_state(journal, lease_id, cleanup_state).await?;
        return Err(MountHelperError::Protocol(
            "mounted snapshot has no usable device identity".into(),
        ));
    }
    let journal_update = sqlx::query(
        "UPDATE mounts SET mount_device = ?, state = 'active' WHERE lease_id = ? AND state = 'prepared'",
    )
        .bind(i64::try_from(mount_device).unwrap_or(i64::MAX))
        .bind(lease_id)
        .execute(journal)
        .await;
    match journal_update {
        Ok(result) if result.rows_affected() == 1 => {}
        Ok(_) | Err(_) => {
            // Never return a live mount after its identity failed to persist. The mount root is
            // root-owned and cannot be replaced by the requesting user during cleanup.
            let cleanup_state = cleanup_mount(
                &mount_root,
                manifest,
                Some(mount_device),
                volume_uuid,
                journal,
                lease_id,
                device_identifier,
                &reservation_manifest_json,
                &stored_manifest_json,
            )
            .await;
            let _ = update_state(journal, lease_id, cleanup_state).await;
            return Err(MountHelperError::Protocol(
                "mounted snapshot identity could not be persisted".into(),
            ));
        }
    }
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
        "SELECT uid, volume_uuid, device_identifier, mount_root, mount_device, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, last_renewed_at FROM mounts WHERE lease_id = ?",
    )
    .bind(lease_id)
    .fetch_optional(journal)
    .await?
    .ok_or_else(|| MountHelperError::Protocol("mount lease not found".into()))?;
    let stored_uid: i64 = row.try_get("uid")?;
    let volume_uuid: String = row.try_get("volume_uuid")?;
    let device_identifier: String = row.try_get("device_identifier")?;
    let mount_root: String = row.try_get("mount_root")?;
    let mount_device: i64 = row.try_get("mount_device").unwrap_or_default();
    let manifest_json: String = row.try_get("snapshot_manifest")?;
    let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
    let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
    let state: String = row.try_get("state")?;
    let last_renewed_at: String = row.try_get("last_renewed_at")?;
    let stale_reserved = matches!(state.as_str(), "reserved" | "cleanup_pending")
        && mount_root.is_empty()
        && manifest_json == "[]"
        && sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM mounts WHERE lease_id = ? AND last_renewed_at = ? AND last_renewed_at <= datetime('now', ?)",
        )
        .bind(lease_id)
        .bind(&last_renewed_at)
        .bind(RECOVERY_AGE_SECONDS)
        .fetch_one(journal)
        .await?
        > 0;
    if stored_uid != i64::from(uid) && !stale_reserved {
        return Err(MountHelperError::Protocol(
            "mount lease belongs to another user".into(),
        ));
    }
    if state == "released" {
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: "released".to_string(),
        });
    }
    if reservation_manifest_valid != 1 {
        update_state(journal, lease_id, "cleanup_pending").await?;
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: "cleanup_pending".to_string(),
        });
    }
    if matches!(state.as_str(), "reserved" | "cleanup_pending")
        && mount_root.is_empty()
        && manifest_json == "[]"
    {
        if !stale_reserved {
            return Ok(MountReleaseResult {
                lease_id: lease_id.to_string(),
                cleanup_state: "cleanup_pending".to_string(),
            });
        }
        let cleanup_state = reconcile_reserved_snapshot(
            journal,
            lease_id,
            &volume_uuid,
            &device_identifier,
            &reservation_manifest_json,
        )
        .await;
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: cleanup_state.to_string(),
        });
    }
    let manifest = match parse_journal_manifest(&manifest_json) {
        Ok(manifest) => manifest,
        Err(_) => {
            update_state(journal, lease_id, "cleanup_pending").await?;
            return Ok(MountReleaseResult {
                lease_id: lease_id.to_string(),
                cleanup_state: "cleanup_pending".to_string(),
            });
        }
    };
    if manifest.iter().any(|snapshot| {
        !snapshot
            .device_identifier
            .eq_ignore_ascii_case(&device_identifier)
    }) || validate_device_volume_binding(&device_identifier, &volume_uuid).is_err()
    {
        update_state(journal, lease_id, "cleanup_pending").await?;
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: "cleanup_pending".to_string(),
        });
    }
    if !inventory_matches_expected(&device_identifier, &reservation_manifest_json, &manifest) {
        update_state(journal, lease_id, "cleanup_pending").await?;
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: "cleanup_pending".to_string(),
        });
    }
    if mount_root.is_empty() {
        let cleanup_state = if delete_manifest_and_record_tombstone(
            journal,
            lease_id,
            &volume_uuid,
            &device_identifier,
            &reservation_manifest_json,
            &manifest_json,
            &manifest,
        )
        .await
            == "released"
        {
            "released"
        } else {
            "cleanup_pending"
        };
        update_state(journal, lease_id, cleanup_state).await?;
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: cleanup_state.to_string(),
        });
    }
    let expected_device = u64::try_from(mount_device)
        .ok()
        .filter(|device| *device != 0);
    let expected_root = expected_mount_root(i64::from(uid), lease_id)?;
    let cleanup_state = if Path::new(&mount_root) != expected_root
        || !validate_managed_mount_root(&expected_root)
    {
        "cleanup_pending"
    } else {
        cleanup_mount(
            &expected_root,
            &manifest,
            expected_device,
            &volume_uuid,
            journal,
            lease_id,
            &device_identifier,
            &reservation_manifest_json,
            &manifest_json,
        )
        .await
    };
    update_state(journal, lease_id, cleanup_state).await?;
    Ok(MountReleaseResult {
        lease_id: lease_id.to_string(),
        cleanup_state: cleanup_state.to_string(),
    })
}

async fn reconcile_reserved_snapshot(
    journal: &SqlitePool,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    reservation_manifest_json: &str,
) -> &'static str {
    let reservation_manifest = match parse_reservation_manifest(reservation_manifest_json) {
        Ok(manifest) => manifest,
        Err(_) => {
            let _ = update_state(journal, lease_id, "cleanup_pending").await;
            return "cleanup_pending";
        }
    };
    if validate_device_volume_binding(device_identifier, volume_uuid).is_err() {
        let _ = update_state(journal, lease_id, "cleanup_pending").await;
        return "cleanup_pending";
    }
    let current_manifest = match snapshot_inventory_manifest(device_identifier) {
        Ok(manifest) => manifest,
        Err(_) => {
            let _ = update_state(journal, lease_id, "cleanup_pending").await;
            return "cleanup_pending";
        }
    };
    if !manifests_equal(&current_manifest, &reservation_manifest) {
        let _ = update_state(journal, lease_id, "cleanup_pending").await;
        return "cleanup_pending";
    }
    let confirmed_manifest = match snapshot_inventory_manifest(device_identifier) {
        Ok(manifest) => manifest,
        Err(_) => {
            let _ = update_state(journal, lease_id, "cleanup_pending").await;
            return "cleanup_pending";
        }
    };
    if !manifests_equal(&confirmed_manifest, &reservation_manifest) {
        let _ = update_state(journal, lease_id, "cleanup_pending").await;
        return "cleanup_pending";
    }
    let updated = sqlx::query(
        // Keep the row as a released tombstone. Future reservations audit this baseline before
        // any source bytes can be served, covering snapshots created after this transition.
        "UPDATE mounts SET state = 'released', reservation_manifest_valid = 1 WHERE lease_id = ? AND state IN ('reserved', 'cleanup_pending') AND mount_root = '' AND snapshot_manifest = '[]' AND reservation_manifest = ? AND reservation_manifest_valid = 1",
    )
    .bind(lease_id)
    .bind(reservation_manifest_json)
    .execute(journal)
    .await;
    if !matches!(updated, Ok(result) if result.rows_affected() == 1) {
        return "cleanup_pending";
    }
    match snapshot_inventory_manifest(device_identifier) {
        // The released row is retained as a tombstone. A later Reserve audits it before serving
        // bytes, so a snapshot created after this check cannot become untracked.
        Ok(final_manifest) if manifests_equal(&final_manifest, &reservation_manifest) => "released",
        _ => {
            let _ = update_state(journal, lease_id, "cleanup_pending").await;
            "cleanup_pending"
        }
    }
}

async fn cleanup_request(
    peer_uid: u32,
    uid: u32,
    cleanup_id: &str,
    journal: &SqlitePool,
) -> Result<MountReleaseResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "cleanup request UID does not match peer".into(),
        ));
    }
    validate_lease_id(cleanup_id)?;
    let row = sqlx::query(
        "SELECT uid, volume_uuid, device_identifier, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state FROM mounts WHERE lease_id = ?",
    )
    .bind(cleanup_id)
    .fetch_optional(journal)
    .await?
    .ok_or_else(|| {
        MountHelperError::Protocol("cleanup requires an existing journal entry".into())
    })?;
    let stored_uid: i64 = row.try_get("uid")?;
    if stored_uid != i64::from(uid) {
        return Err(MountHelperError::Protocol(
            "cleanup journal entry belongs to another user".into(),
        ));
    }
    let state: String = row.try_get("state")?;
    if state != "prepared" {
        return Err(MountHelperError::Protocol(
            "cleanup requires a prepared journal entry".into(),
        ));
    }
    let mount_root: String = row.try_get("mount_root")?;
    if !mount_root.is_empty() {
        return Err(MountHelperError::Protocol(
            "mounted leases must be released through the lease operation".into(),
        ));
    }
    let stored_manifest_json: String = row.try_get("snapshot_manifest")?;
    let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
    let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
    let stored_manifest = parse_journal_manifest(&stored_manifest_json)?;
    let volume_uuid: String = row.try_get("volume_uuid")?;
    let device_identifier: String = row.try_get("device_identifier")?;
    if reservation_manifest_valid != 1 {
        return Err(MountHelperError::Protocol(
            "cleanup journal entry has no auditable baseline".into(),
        ));
    }
    if stored_manifest.iter().any(|snapshot| {
        !snapshot
            .device_identifier
            .eq_ignore_ascii_case(&device_identifier)
    }) || validate_device_volume_binding(&device_identifier, &volume_uuid).is_err()
    {
        return Err(MountHelperError::Protocol(
            "cleanup device identity no longer matches the journaled volume".into(),
        ));
    }
    if !inventory_matches_expected(
        &device_identifier,
        &reservation_manifest_json,
        &stored_manifest,
    ) {
        update_state(journal, cleanup_id, "cleanup_pending").await?;
        return Ok(MountReleaseResult {
            lease_id: cleanup_id.to_string(),
            cleanup_state: "cleanup_pending".to_string(),
        });
    }
    let cleanup_state = if delete_manifest_and_record_tombstone(
        journal,
        cleanup_id,
        &volume_uuid,
        &device_identifier,
        &reservation_manifest_json,
        &stored_manifest_json,
        &stored_manifest,
    )
    .await
        == "released"
    {
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

#[allow(clippy::too_many_arguments)]
async fn cleanup_mount(
    mount_root: &Path,
    manifest: &[MountSnapshotRef],
    expected_device: Option<u64>,
    volume_uuid: &str,
    journal: &SqlitePool,
    lease_id: &str,
    device_identifier: &str,
    reservation_manifest_json: &str,
    manifest_json: &str,
) -> &'static str {
    let unmount_ok = match mounted_snapshot_path(mount_root) {
        Ok(true)
            if expected_device.is_none_or(|expected| {
                fs::metadata(mount_root)
                    .map(|metadata| metadata.dev() == expected)
                    .unwrap_or(false)
            }) =>
        {
            run_command(
                "snapshot_unmount",
                "umount",
                "/sbin/umount",
                ["-f", mount_root.to_string_lossy().as_ref()],
            )
            .is_ok()
                && matches!(mounted_snapshot_path(mount_root), Ok(false))
        }
        // Never unmount a different device that was substituted at the recorded path.
        Ok(true) => false,
        // A prepared journal row can survive a crash before mount_apfs completes. In that case
        // there is no mount to remove, so UUID-scoped snapshot cleanup can proceed.
        Ok(false) => true,
        Err(_) => false,
    };
    if !unmount_ok {
        return "cleanup_pending";
    }
    if fs::remove_dir(mount_root).is_err() && mount_root.exists() {
        return "cleanup_pending";
    }
    delete_manifest_and_record_tombstone(
        journal,
        lease_id,
        volume_uuid,
        device_identifier,
        reservation_manifest_json,
        manifest_json,
        manifest,
    )
    .await
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

async fn delete_manifest_and_record_tombstone(
    journal: &SqlitePool,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    reservation_manifest_json: &str,
    manifest_json: &str,
    manifest: &[MountSnapshotRef],
) -> &'static str {
    if !inventory_matches_expected(device_identifier, reservation_manifest_json, manifest) {
        return "cleanup_pending";
    }
    let confirmed_deleted = delete_manifest(volume_uuid, manifest).err();
    if let Some(confirmed_deleted) = confirmed_deleted {
        retain_unconfirmed_manifest_after_delete_failure(
            journal,
            lease_id,
            device_identifier,
            manifest_json,
            manifest,
            &confirmed_deleted,
        )
        .await;
        return "cleanup_pending";
    }
    let current_manifest = match snapshot_inventory_manifest(device_identifier) {
        Ok(manifest) => manifest,
        Err(_) => {
            let _ = sqlx::query(
                "UPDATE mounts SET snapshot_manifest = '[]', state = 'cleanup_pending' WHERE lease_id = ? AND snapshot_manifest = ?",
            )
            .bind(lease_id)
            .bind(manifest_json)
            .execute(journal)
            .await;
            return "cleanup_pending";
        }
    };
    let reservation_manifest = match parse_reservation_manifest(reservation_manifest_json) {
        Ok(manifest) => manifest,
        Err(_) => {
            let _ = sqlx::query(
                "UPDATE mounts SET snapshot_manifest = '[]', state = 'cleanup_pending' WHERE lease_id = ? AND snapshot_manifest = ?",
            )
            .bind(lease_id)
            .bind(manifest_json)
            .execute(journal)
            .await;
            return "cleanup_pending";
        }
    };
    // A post-deletion inventory change is unowned; keep the original baseline and block reuse.
    if !manifests_equal(&current_manifest, &reservation_manifest) {
        let _ = sqlx::query(
            "UPDATE mounts SET snapshot_manifest = '[]', state = 'cleanup_pending' WHERE lease_id = ? AND snapshot_manifest = ?",
        )
        .bind(lease_id)
        .bind(manifest_json)
        .execute(journal)
        .await;
        return "cleanup_pending";
    }
    let updated = sqlx::query(
        "UPDATE mounts SET snapshot_manifest = '[]', reservation_manifest = ?, reservation_manifest_valid = 1, state = 'released' WHERE lease_id = ? AND snapshot_manifest = ? AND reservation_manifest_valid = 1",
    )
    .bind(reservation_manifest_json)
    .bind(lease_id)
    .bind(manifest_json)
    .execute(journal)
    .await;
    if matches!(updated, Ok(result) if result.rows_affected() == 1) {
        "released"
    } else {
        let _ = sqlx::query(
            "UPDATE mounts SET snapshot_manifest = '[]', state = 'cleanup_pending' WHERE lease_id = ?",
        )
        .bind(lease_id)
        .execute(journal)
        .await;
        "cleanup_pending"
    }
}

async fn retain_unconfirmed_manifest_after_delete_failure(
    journal: &SqlitePool,
    lease_id: &str,
    device_identifier: &str,
    manifest_json: &str,
    manifest: &[MountSnapshotRef],
    confirmed_deleted: &[MountSnapshotRef],
) {
    let remaining_manifest = match snapshot_inventory_manifest(device_identifier) {
        Ok(current_manifest) => manifest_entries_remaining_after_partial_delete(
            manifest,
            confirmed_deleted,
            &current_manifest,
        ),
        Err(_) => manifest_entries_remaining_after_partial_delete(manifest, confirmed_deleted, &[]),
    };
    let remaining_manifest_json =
        serde_json::to_string(&remaining_manifest).unwrap_or_else(|_| manifest_json.to_string());
    let _ = sqlx::query(
        "UPDATE mounts SET snapshot_manifest = ?, state = 'cleanup_pending' WHERE lease_id = ? AND snapshot_manifest = ?",
    )
    .bind(remaining_manifest_json)
    .bind(lease_id)
    .bind(manifest_json)
    .execute(journal)
    .await;
}

fn manifest_entries_remaining_after_partial_delete(
    recorded_manifest: &[MountSnapshotRef],
    confirmed_deleted: &[MountSnapshotRef],
    current_manifest: &[MountSnapshotRef],
) -> Vec<MountSnapshotRef> {
    recorded_manifest
        .iter()
        .filter(|recorded| {
            !confirmed_deleted.iter().any(|deleted| {
                deleted
                    .device_identifier
                    .eq_ignore_ascii_case(&recorded.device_identifier)
                    && deleted.uuid.eq_ignore_ascii_case(&recorded.uuid)
            }) || current_manifest.iter().any(|current| {
                current
                    .device_identifier
                    .eq_ignore_ascii_case(&recorded.device_identifier)
                    && current.uuid.eq_ignore_ascii_case(&recorded.uuid)
            })
        })
        .cloned()
        .collect()
}

fn delete_manifest(
    volume_uuid: &str,
    manifest: &[MountSnapshotRef],
) -> Result<(), Vec<MountSnapshotRef>> {
    let Some(device_identifier) = manifest
        .first()
        .map(|snapshot| snapshot.device_identifier.as_str())
    else {
        return Err(Vec::new());
    };
    if manifest.iter().any(|snapshot| {
        !snapshot
            .device_identifier
            .eq_ignore_ascii_case(device_identifier)
    }) {
        return Err(Vec::new());
    }
    let mut confirmed_deleted = Vec::new();
    let mut failed = false;
    for snapshot in manifest {
        if validate_device_volume_binding(device_identifier, volume_uuid).is_err() {
            return Err(confirmed_deleted);
        }
        if run_command(
            "snapshot_delete",
            "diskutil.apfs.deleteSnapshot",
            "/usr/sbin/diskutil",
            [
                "apfs",
                "deleteSnapshot",
                &snapshot.device_identifier,
                "-uuid",
                &snapshot.uuid,
            ],
        )
        .is_err()
        {
            failed = true;
        } else {
            confirmed_deleted.push(snapshot.clone());
        }
    }
    if failed {
        Err(confirmed_deleted)
    } else {
        Ok(())
    }
}

fn parse_journal_manifest(value: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
    let manifest = serde_json::from_str::<Vec<MountSnapshotRef>>(value).map_err(|_| {
        MountHelperError::Protocol("snapshot mount journal contains an invalid manifest".into())
    })?;
    if manifest.is_empty() {
        return Err(MountHelperError::Protocol(
            "snapshot mount journal contains an empty manifest".into(),
        ));
    }
    for snapshot in &manifest {
        validate_device(&snapshot.device_identifier)?;
        validate_uuid(&snapshot.uuid)?;
    }
    Ok(manifest)
}

fn parse_reservation_manifest(value: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
    let manifest = serde_json::from_str::<Vec<MountSnapshotRef>>(value).map_err(|_| {
        MountHelperError::Protocol("snapshot mount journal contains an invalid reservation".into())
    })?;
    for snapshot in &manifest {
        validate_device(&snapshot.device_identifier)?;
        validate_uuid(&snapshot.uuid)?;
    }
    Ok(manifest)
}

fn manifests_equal(left: &[MountSnapshotRef], right: &[MountSnapshotRef]) -> bool {
    let mut left_keys: Vec<_> = left
        .iter()
        .map(|snapshot| {
            (
                snapshot.device_identifier.to_ascii_lowercase(),
                snapshot.uuid.to_ascii_lowercase(),
            )
        })
        .collect();
    let mut right_keys: Vec<_> = right
        .iter()
        .map(|snapshot| {
            (
                snapshot.device_identifier.to_ascii_lowercase(),
                snapshot.uuid.to_ascii_lowercase(),
            )
        })
        .collect();
    left_keys.sort_unstable();
    right_keys.sort_unstable();
    left_keys == right_keys
}

fn inventory_matches_expected(
    device_identifier: &str,
    reservation_manifest_json: &str,
    recorded_manifest: &[MountSnapshotRef],
) -> bool {
    let Ok(reservation_manifest) = parse_reservation_manifest(reservation_manifest_json) else {
        return false;
    };
    let Ok(current_manifest) = snapshot_inventory_manifest(device_identifier) else {
        return false;
    };
    let mut expected_manifest = reservation_manifest;
    expected_manifest.extend(recorded_manifest.iter().cloned());
    manifests_equal(&current_manifest, &expected_manifest)
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

fn validate_source_mount(
    source_mount_point: &Path,
    expected_volume_uuid: &str,
    expected_device: &str,
) -> Result<PathBuf, MountHelperError> {
    if !source_mount_point.is_dir() {
        return Err(MountHelperError::Protocol(
            "source mount point is not a directory".into(),
        ));
    }
    validate_source_path(source_mount_point, expected_volume_uuid, expected_device)?;
    // Pass the validated device node to mount_apfs instead of the request pathname. The
    // pathname is only used for the diskutil identity query; a user cannot replace the
    // root-owned /dev node between validation and the privileged mount operation.
    Ok(PathBuf::from("/dev").join(expected_device))
}

fn validate_source_path(
    source_path: &Path,
    expected_volume_uuid: &str,
    expected_device: &str,
) -> Result<SourceDirectory, MountHelperError> {
    let source = SourceDirectory::open(source_path)?;
    if source.device_identifier()? != expected_device {
        return Err(MountHelperError::Protocol(
            "source mount identity does not match the lease".into(),
        ));
    }
    // diskutil accepts a volume/device, not an arbitrary directory. Use only
    // the kernel's device identity from the pinned directory descriptor.
    validate_device_volume_binding_at("source_volume", expected_device, expected_volume_uuid)?;
    source.recheck(source_path)?;
    Ok(source)
}

struct SourceDirectory {
    directory: fs::File,
    device: u64,
    inode: u64,
}

impl SourceDirectory {
    fn open(path: &Path) -> Result<Self, MountHelperError> {
        if !path.is_absolute() {
            return Err(MountHelperError::Protocol(
                "source path must be absolute".into(),
            ));
        }
        // Open each component relative to its predecessor. No user-controlled
        // symlink (including an ancestor) can redirect root's metadata query.
        let mut directory = open_directory_at(libc::AT_FDCWD, std::ffi::OsStr::new("/"))?;
        for component in path.components().skip(1) {
            let std::path::Component::Normal(name) = component else {
                return Err(MountHelperError::Protocol(
                    "invalid source path component".into(),
                ));
            };
            directory = open_directory_at(directory.as_raw_fd(), name)?;
        }
        let metadata = directory.metadata()?;
        Ok(Self {
            directory,
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }

    fn recheck(&self, path: &Path) -> Result<(), MountHelperError> {
        let current = Self::open(path)?;
        if self.device != current.device
            || self.inode != current.inode
            || self.device_identifier()? != current.device_identifier()?
        {
            return Err(MountHelperError::Protocol(
                "source directory identity changed".into(),
            ));
        }
        Ok(())
    }

    fn device_identifier(&self) -> Result<String, MountHelperError> {
        #[cfg(target_os = "macos")]
        {
            let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
            if unsafe { libc::fstatfs(self.directory.as_raw_fd(), filesystem.as_mut_ptr()) } != 0 {
                return Err(MountHelperError::Io(std::io::Error::last_os_error()));
            }
            let filesystem = unsafe { filesystem.assume_init() };
            let bytes: Vec<u8> = filesystem
                .f_mntfromname
                .iter()
                .take_while(|byte| **byte != 0)
                .map(|byte| *byte as u8)
                .collect();
            let name = std::str::from_utf8(&bytes).map_err(|_| {
                MountHelperError::Protocol("invalid source filesystem device".into())
            })?;
            let device = name.strip_prefix("/dev/").ok_or_else(|| {
                MountHelperError::Protocol("source filesystem is not a disk device".into())
            })?;
            validate_device(device)?;
            Ok(device.to_owned())
        }
        #[cfg(not(target_os = "macos"))]
        {
            // The privileged service is macOS-only; never guess a device elsewhere.
            let _ = self.directory.as_raw_fd();
            Err(MountHelperError::Protocol(
                "source filesystem inspection requires macOS".into(),
            ))
        }
    }
}

fn open_directory_at(parent: RawFd, name: &std::ffi::OsStr) -> Result<fs::File, MountHelperError> {
    let name = CString::new(name.as_bytes())
        .map_err(|_| MountHelperError::Protocol("invalid source path".into()))?;
    #[cfg(target_os = "macos")]
    let metadata_only = libc::O_EVTONLY;
    #[cfg(not(target_os = "macos"))]
    let metadata_only = libc::O_PATH;
    let descriptor = unsafe {
        libc::openat(
            parent,
            name.as_ptr(),
            metadata_only | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if descriptor < 0 {
        return Err(MountHelperError::Io(std::io::Error::last_os_error()));
    }
    Ok(unsafe { fs::File::from_raw_fd(descriptor) })
}

fn authorize_source_path(
    uid: u32,
    source_path: &Path,
    source: &SourceDirectory,
) -> Result<(), MountHelperError> {
    if uid == 0 || unsafe { libc::geteuid() } != 0 {
        return Err(MountHelperError::Protocol(
            "source path authorization requires a non-root peer".into(),
        ));
    }
    let executable = std::env::current_exe()?;
    let status = Command::new(executable)
        .arg(AUTHORIZE_SOURCE_PATH_ARG)
        .arg(uid.to_string())
        .arg(source_path)
        .arg(source.device.to_string())
        .arg(source.inode.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(MountHelperError::Protocol(
            "peer cannot access the requested source path".into(),
        ))
    }
}

/// Run source-path authorization before the Tokio runtime starts.
///
/// This is an exec'd process rather than a `fork()` child of the multithreaded server, so
/// privilege transitions cannot inherit locks held by threads that do not exist in the child.
pub fn maybe_authorize_source_path(args: &[String]) -> Option<i32> {
    if args.get(1).map(String::as_str) != Some(AUTHORIZE_SOURCE_PATH_ARG) {
        return None;
    }
    if args.len() != 6 {
        return Some(126);
    }
    let Some(uid) = args.get(2).and_then(|value| value.parse::<u32>().ok()) else {
        return Some(126);
    };
    let Some(path) = args.get(3).map(Path::new).filter(|path| path.is_absolute()) else {
        return Some(126);
    };
    let Some(device) = args[4].parse::<u64>().ok() else {
        return Some(126);
    };
    let Some(inode) = args[5].parse::<u64>().ok() else {
        return Some(126);
    };
    Some(authorize_source_path_child(uid, path, device, inode))
}

fn authorize_source_path_child(uid: u32, source_path: &Path, device: u64, inode: u64) -> i32 {
    if uid == 0 || unsafe { libc::geteuid() } != 0 {
        return 126;
    }
    let path = match CString::new(source_path.as_os_str().as_bytes()) {
        Ok(path) => path,
        Err(_) => return 126,
    };
    let passwd = unsafe { libc::getpwuid(uid as libc::uid_t) };
    if passwd.is_null() {
        return 126;
    }
    let (group, user) = unsafe { ((*passwd).pw_gid, (*passwd).pw_name) };
    if unsafe { libc::setgid(group) } != 0
        || !initialize_supplementary_groups(user, group)
        || unsafe { libc::setuid(uid as libc::uid_t) } != 0
    {
        return 126;
    }
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if fd < 0 {
        return 1;
    }
    let directory = unsafe { fs::File::from_raw_fd(fd) };
    if directory
        .metadata()
        .is_ok_and(|metadata| metadata.dev() == device && metadata.ino() == inode)
    {
        0
    } else {
        1
    }
}

fn initialize_supplementary_groups(user: *const libc::c_char, group: libc::gid_t) -> bool {
    #[cfg(target_os = "macos")]
    {
        let Ok(base_group) = libc::c_int::try_from(group) else {
            return false;
        };
        unsafe { libc::initgroups(user, base_group) == 0 }
    }
    #[cfg(not(target_os = "macos"))]
    {
        unsafe { libc::initgroups(user, group) == 0 }
    }
}

fn validate_device_volume_binding(
    device_identifier: &str,
    expected_volume_uuid: &str,
) -> Result<(), MountHelperError> {
    validate_device_volume_binding_at("volume_identity", device_identifier, expected_volume_uuid)
}

fn validate_device_volume_binding_at(
    stage: &'static str,
    device_identifier: &str,
    expected_volume_uuid: &str,
) -> Result<(), MountHelperError> {
    let device_path = format!("/dev/{device_identifier}");
    let output = run_command_output(
        stage,
        "diskutil.info",
        "/usr/sbin/diskutil",
        ["info", "-plist", device_path.as_str()],
    )?;
    let value = Value::from_reader_xml(output.as_slice()).map_err(|_| {
        MountHelperError::Protocol(format!(
            "stage={stage} command=diskutil.info exit=0 channel=stdout summary=invalid volume plist"
        ))
    })?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| MountHelperError::Protocol(format!("stage={stage} command=diskutil.info exit=0 channel=stdout summary=volume plist is not a dictionary")))?;
    let filesystem = dict
        .get("FilesystemName")
        .and_then(Value::as_string)
        .unwrap_or_default();
    let actual_device = dict
        .get("DeviceIdentifier")
        .and_then(Value::as_string)
        .unwrap_or_default();
    let actual_volume_uuid = dict
        .get("APFSVolumeUUID")
        .and_then(Value::as_string)
        .or_else(|| dict.get("VolumeUUID").and_then(Value::as_string))
        .unwrap_or_default();
    if !filesystem.eq_ignore_ascii_case("APFS")
        || !actual_device.eq_ignore_ascii_case(device_identifier)
        || !actual_volume_uuid.eq_ignore_ascii_case(expected_volume_uuid)
    {
        return Err(MountHelperError::Protocol(
            "device does not match the requested APFS volume".into(),
        ));
    }
    Ok(())
}

fn validate_snapshot_binding(
    device_identifier: &str,
    snapshot_uuid: &str,
    snapshot_name: &str,
) -> Result<(), MountHelperError> {
    let output = run_command_output(
        "snapshot_inventory",
        "diskutil.apfs.listSnapshots",
        "/usr/sbin/diskutil",
        ["apfs", "listSnapshots", "-plist", device_identifier],
    )?;
    let value = Value::from_reader_xml(output.as_slice())
        .map_err(|_| MountHelperError::Protocol("stage=snapshot_inventory command=diskutil.apfs.listSnapshots exit=0 channel=stdout summary=invalid snapshot plist".into()))?;
    if snapshot_binding_matches(&value, snapshot_uuid, snapshot_name) {
        Ok(())
    } else {
        Err(MountHelperError::Protocol(
            "snapshot name does not match the requested snapshot UUID".into(),
        ))
    }
}

fn snapshot_inventory_manifest(
    device_identifier: &str,
) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
    let output = run_command_output(
        "snapshot_inventory",
        "diskutil.apfs.listSnapshots",
        "/usr/sbin/diskutil",
        ["apfs", "listSnapshots", "-plist", device_identifier],
    )?;
    parse_snapshot_inventory(&output, device_identifier)
}

fn parse_snapshot_inventory(
    output: &[u8],
    device_identifier: &str,
) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
    let value = Value::from_reader_xml(output)
        .map_err(|_| MountHelperError::Protocol("stage=snapshot_inventory command=diskutil.apfs.listSnapshots exit=0 channel=stdout summary=invalid snapshot plist".into()))?;
    let Some(snapshots) = value
        .as_dictionary()
        .and_then(|root| root.get("Snapshots"))
        .and_then(Value::as_array)
    else {
        return Err(MountHelperError::Protocol(
            "stage=snapshot_inventory command=diskutil.apfs.listSnapshots exit=0 channel=stdout summary=snapshot inventory plist is missing an array of snapshots".into(),
        ));
    };
    let mut manifest = Vec::new();
    for snapshot in snapshots {
        let snapshot = snapshot.as_dictionary().ok_or_else(|| {
            MountHelperError::Protocol("stage=snapshot_inventory command=diskutil.apfs.listSnapshots exit=0 channel=stdout summary=snapshot inventory contains a malformed entry".into())
        })?;
        let uuid = snapshot
            .get("SnapshotUUID")
            .and_then(Value::as_string)
            .ok_or_else(|| {
                MountHelperError::Protocol(
                    "stage=snapshot_inventory command=diskutil.apfs.listSnapshots exit=0 channel=stdout summary=snapshot inventory entry is missing SnapshotUUID".into(),
                )
            })?;
        validate_uuid(uuid)?;
        manifest.push(MountSnapshotRef {
            device_identifier: device_identifier.to_string(),
            uuid: uuid.to_string(),
        });
    }
    Ok(manifest)
}

fn snapshot_binding_matches(value: &Value, snapshot_uuid: &str, snapshot_name: &str) -> bool {
    value
        .as_dictionary()
        .and_then(|root| root.get("Snapshots"))
        .and_then(Value::as_array)
        .is_some_and(|snapshots| {
            snapshots.iter().any(|snapshot| {
                let Some(snapshot) = snapshot.as_dictionary() else {
                    return false;
                };
                snapshot
                    .get("SnapshotUUID")
                    .and_then(Value::as_string)
                    .is_some_and(|uuid| uuid.eq_ignore_ascii_case(snapshot_uuid))
                    && snapshot.get("SnapshotName").and_then(Value::as_string)
                        == Some(snapshot_name)
            })
        })
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

fn ensure_root_owned_file(path: &Path, description: &str) -> Result<(), MountHelperError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != 0
        || metadata.mode() & 0o022 != 0
    {
        return Err(MountHelperError::Protocol(format!(
            "unsafe root-owned {description}: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_root_file(path: &Path) -> Result<(), MountHelperError> {
    ensure_root_owned_file(path, "mount helper journal")
}

fn run_command<const N: usize, S: AsRef<std::ffi::OsStr>>(
    stage: &'static str,
    command: &'static str,
    program: &str,
    args: [S; N],
) -> Result<(), MountHelperError> {
    run_command_output(stage, command, program, args).map(|_| ())
}

fn run_command_output<const N: usize, S: AsRef<std::ffi::OsStr>>(
    stage: &'static str,
    command: &'static str,
    program: &str,
    args: [S; N],
) -> Result<Vec<u8>, MountHelperError> {
    let output =
        Command::new(program)
            .args(args)
            .output()
            .map_err(|error| MountHelperError::Command {
                stage,
                command,
                status: "not_started".into(),
                channel: "errno",
                summary: format!("errno={}", error.raw_os_error().unwrap_or(0)),
            })?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(command_failure(stage, command, &output))
}

fn command_failure(
    stage: &'static str,
    command: &'static str,
    output: &std::process::Output,
) -> MountHelperError {
    let (channel, bytes) = if !output.stderr.iter().all(u8::is_ascii_whitespace) {
        ("stderr", output.stderr.as_slice())
    } else if !output.stdout.iter().all(u8::is_ascii_whitespace) {
        ("stdout", output.stdout.as_slice())
    } else {
        ("none", &[][..])
    };
    MountHelperError::Command {
        stage,
        command,
        status: output
            .status
            .code()
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".into()),
        channel,
        summary: sanitized_command_summary(bytes),
    }
}

fn sanitized_command_summary(bytes: &[u8]) -> String {
    // OS diagnostics can contain paths, volume labels, UUIDs, URLs, or secrets.
    // Project known error classes onto fixed text; never echo arbitrary output.
    // Inspect before truncation so a long private prefix cannot hide the cause.
    let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
    let summary = [
        ("could not find disk", "could not find disk"),
        ("permission denied", "permission denied"),
        ("operation not permitted", "operation not permitted"),
        ("resource busy", "resource busy"),
        ("input/output error", "input/output error"),
        ("no such file", "no such file or directory"),
        ("not mounted", "volume not mounted"),
        ("invalid argument", "invalid argument"),
        ("timed out", "command timed out"),
        ("read-only", "read-only filesystem"),
    ]
    .into_iter()
    .find_map(|(pattern, safe)| text.contains(pattern).then_some(safe))
    .unwrap_or(if text.trim().is_empty() {
        "no diagnostic output"
    } else {
        "unrecognized diagnostic redacted"
    });
    summary.chars().take(512).collect()
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
    fn command_errors_keep_stage_exit_channel_and_only_safe_summary() {
        use std::os::unix::process::ExitStatusExt;
        for (stdout, stderr, channel, summary) in [
            (
                b"Could not find disk: /private/synthetic-secret?token=SECRET".as_slice(),
                b"".as_slice(),
                "stdout",
                "could not find disk",
            ),
            (
                b"".as_slice(),
                b"Permission denied: /private/synthetic-secret token=SECRET".as_slice(),
                "stderr",
                "permission denied",
            ),
            (
                b"".as_slice(),
                b"".as_slice(),
                "none",
                "no diagnostic output",
            ),
            (
                b"<plist><string>SECRET</string>".as_slice(),
                b"".as_slice(),
                "stdout",
                "unrecognized diagnostic redacted",
            ),
        ] {
            let output = std::process::Output {
                status: std::process::ExitStatus::from_raw(256),
                stdout: stdout.to_vec(),
                stderr: stderr.to_vec(),
            };
            let message = command_failure("source_volume", "diskutil.info", &output).to_string();
            assert!(message.contains("stage=source_volume command=diskutil.info exit=1"));
            assert!(message.contains(&format!("channel={channel} summary={summary}")));
            assert!(!message.contains("SECRET") && !message.contains("/private"));
        }
        let long = format!(
            "{} Permission denied /private/SECRET",
            "sensitive".repeat(200)
        );
        assert_eq!(
            sanitized_command_summary(long.as_bytes()),
            "permission denied"
        );
        assert!(sanitized_command_summary(long.as_bytes()).chars().count() <= 512);
    }

    #[test]
    fn snapshot_inventory_accepts_explicit_empty_array_and_rejects_corrupt_data() {
        let empty = br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Snapshots</key><array/></dict></plist>"#;
        assert!(
            parse_snapshot_inventory(empty, "disk5s1")
                .unwrap()
                .is_empty()
        );
        for corrupt in [
            b"SECRET /private/source".as_slice(),
            b"<plist><dict/></plist>".as_slice(),
            b"<plist><dict><key>Snapshots</key><string>SECRET</string></dict></plist>".as_slice(),
        ] {
            let message = parse_snapshot_inventory(corrupt, "disk5s1")
                .unwrap_err()
                .to_string();
            assert!(!message.contains("SECRET") && !message.contains("/private/source"));
        }
    }

    #[test]
    fn source_directory_rejects_symlinks_missing_paths_and_replacement() {
        let parent = std::env::temp_dir()
            .canonicalize()
            .unwrap()
            .join(format!("source-identity-{}", Uuid::new_v4()));
        fs::create_dir(&parent).unwrap();
        let source = parent.join("source");
        fs::create_dir(&source).unwrap();
        let pinned = SourceDirectory::open(&source).unwrap();
        let link = parent.join("link");
        std::os::unix::fs::symlink(&source, &link).unwrap();
        assert!(SourceDirectory::open(&link).is_err());
        assert!(SourceDirectory::open(&link.join("child")).is_err());
        assert!(SourceDirectory::open(&parent.join("missing")).is_err());
        assert!(SourceDirectory::open(Path::new("relative")).is_err());
        fs::rename(&source, parent.join("original")).unwrap();
        fs::create_dir(&source).unwrap();
        assert!(pinned.recheck(&source).is_err());
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "requires a disposable GitHub-hosted APFS image"]
    fn source_directory_rejects_real_mount_identity_loss() {
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        struct Fixture {
            root: PathBuf,
            attached: bool,
        }
        impl Drop for Fixture {
            fn drop(&mut self) {
                if self.attached {
                    let stopped = Command::new("hdiutil")
                        .args(["detach", "-force"])
                        .arg(self.root.join("volume"))
                        .output()
                        .is_ok_and(|output| output.status.success());
                    if !stopped {
                        return;
                    }
                }
                let _ = fs::remove_dir_all(&self.root);
            }
        }
        let mut fixture = Fixture {
            root: std::env::temp_dir()
                .canonicalize()
                .unwrap()
                .join(format!("mount-identity-{}", Uuid::new_v4())),
            attached: false,
        };
        fs::create_dir(&fixture.root).unwrap();
        let mount = fixture.root.join("volume");
        fs::create_dir(&mount).unwrap();
        let image = fixture.root.join("fixture.dmg");
        assert!(
            Command::new("hdiutil")
                .args([
                    "create",
                    "-size",
                    "128m",
                    "-fs",
                    "APFS",
                    "-volname",
                    "SyntheticIdentity"
                ])
                .arg(&image)
                .output()
                .unwrap()
                .status
                .success()
        );
        assert!(
            Command::new("hdiutil")
                .args(["attach", "-nobrowse", "-mountpoint"])
                .arg(&mount)
                .arg(&image)
                .output()
                .unwrap()
                .status
                .success()
        );
        fixture.attached = true;
        let pinned = SourceDirectory::open(&mount).unwrap();
        assert!(pinned.recheck(&mount).is_ok());
        assert!(
            Command::new("hdiutil")
                .args(["detach", "-force"])
                .arg(&mount)
                .output()
                .unwrap()
                .status
                .success()
        );
        fixture.attached = false;
        assert!(pinned.recheck(&mount).is_err());
    }

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
    fn journal_manifest_rejects_corrupt_or_empty_cleanup_identity() {
        assert!(parse_journal_manifest("not-json").is_err());
        assert!(parse_journal_manifest("[]").is_err());
        assert!(
            parse_journal_manifest(
                r#"[{"device_identifier":"disk5s1","uuid":"B8A38394-49AE-42F9-85FB-14DD9EA549FA"}]"#
            )
            .is_ok()
        );
    }

    #[test]
    fn snapshot_binding_requires_the_requested_uuid_and_name_pair() {
        let value = Value::from_reader_xml(
            br#"<?xml version="1.0"?><plist version="1.0"><dict><key>Snapshots</key><array><dict><key>SnapshotUUID</key><string>ABC</string><key>SnapshotName</key><string>snapshot-a</string></dict><dict><key>SnapshotUUID</key><string>DEF</string><key>SnapshotName</key><string>snapshot-b</string></dict></array></dict></plist>"#.as_slice(),
        )
        .unwrap();

        assert!(snapshot_binding_matches(&value, "abc", "snapshot-a"));
        assert!(!snapshot_binding_matches(&value, "abc", "snapshot-b"));
        assert!(!snapshot_binding_matches(&value, "missing", "snapshot-a"));
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
            request_timeout(&MountMethod::Renew {
                uid: 501,
                lease_id: "lease".into(),
            }),
            MUTATING_REQUEST_TIMEOUT
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

    #[tokio::test]
    async fn mount_journal_migration_adds_recorded_device_identity() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        sqlx::query(
            "CREATE TABLE mounts (lease_id TEXT PRIMARY KEY, uid INTEGER NOT NULL, volume_uuid TEXT NOT NULL, device_identifier TEXT NOT NULL, snapshot_name TEXT NOT NULL, mount_root TEXT NOT NULL, snapshot_manifest TEXT NOT NULL, state TEXT NOT NULL, created_at TEXT NOT NULL)",
        )
        .execute(&journal)
        .await
        .unwrap();

        init_journal(&journal).await.unwrap();

        let column: Option<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('mounts') WHERE name = 'mount_device'",
        )
        .fetch_optional(&journal)
        .await
        .unwrap();
        assert_eq!(column.as_deref(), Some("mount_device"));

        let reservation_column: Option<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('mounts') WHERE name = 'reservation_manifest'",
        )
        .fetch_optional(&journal)
        .await
        .unwrap();
        assert_eq!(reservation_column.as_deref(), Some("reservation_manifest"));

        let reservation_valid_column: Option<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('mounts') WHERE name = 'reservation_manifest_valid'",
        )
        .fetch_optional(&journal)
        .await
        .unwrap();
        assert_eq!(
            reservation_valid_column.as_deref(),
            Some("reservation_manifest_valid")
        );

        let renewal_column: Option<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('mounts') WHERE name = 'last_renewed_at'",
        )
        .fetch_optional(&journal)
        .await
        .unwrap();
        assert_eq!(renewal_column.as_deref(), Some("last_renewed_at"));

        let source_path_column: Option<String> = sqlx::query_scalar(
            "SELECT name FROM pragma_table_info('mounts') WHERE name = 'source_path'",
        )
        .fetch_optional(&journal)
        .await
        .unwrap();
        assert_eq!(source_path_column.as_deref(), Some("source_path"));
    }

    #[tokio::test]
    async fn recovery_preserves_an_unmaterialized_reservation() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at) VALUES (?, ?, ?, ?, '', '', '[]', 'reserved', datetime('now'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .execute(&journal)
        .await
        .unwrap();

        recover_journal(&journal).await.unwrap();

        let state: String = sqlx::query_scalar("SELECT state FROM mounts WHERE lease_id = ?")
            .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
            .fetch_one(&journal)
            .await
            .unwrap();
        assert_eq!(state, "reserved");
    }

    #[tokio::test]
    async fn status_exposes_only_age_gated_reserved_recovery_leases() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at) VALUES (?, ?, ?, ?, '', '', '[]', 'reserved', datetime('now', '-301 seconds'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .execute(&journal)
        .await
        .unwrap();

        let status =
            status_for_volume_result(501, 501, "C8A38394-49AE-42F9-85FB-14DD9EA549FA", &journal)
                .await
                .unwrap();
        assert_eq!(status.recovery_leases.len(), 1);
        assert_eq!(
            status.recovery_leases[0].lease_id,
            "A8A38394-49AE-42F9-85FB-14DD9EA549FA"
        );
    }

    #[tokio::test]
    async fn status_exposes_a_stale_reserved_lease_without_matching_owner_uid() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at) VALUES (?, ?, ?, ?, '', '', '[]', 'reserved', datetime('now', '-301 seconds'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(502_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .execute(&journal)
        .await
        .unwrap();

        let status =
            status_for_volume_result(501, 501, "C8A38394-49AE-42F9-85FB-14DD9EA549FA", &journal)
                .await
                .unwrap();
        assert_eq!(status.recovery_leases.len(), 1);
    }

    #[test]
    fn manifest_comparison_is_order_and_case_insensitive() {
        let left = vec![
            MountSnapshotRef {
                device_identifier: "disk5s1".into(),
                uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
            },
            MountSnapshotRef {
                device_identifier: "disk5s1".into(),
                uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
            },
        ];
        let right = vec![
            MountSnapshotRef {
                device_identifier: "DISK5S1".into(),
                uuid: "a8a38394-49ae-42f9-85fb-14dd9ea549fa".into(),
            },
            MountSnapshotRef {
                device_identifier: "DISK5S1".into(),
                uuid: "b8a38394-49ae-42f9-85fb-14dd9ea549fa".into(),
            },
        ];
        assert!(manifests_equal(&left, &right));
    }

    #[test]
    fn post_deletion_inventory_rejects_an_unowned_snapshot() {
        let baseline = vec![MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        }];
        let mut changed = baseline.clone();
        changed.push(MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        });

        assert!(!manifests_equal(&changed, &baseline));
    }

    #[test]
    fn partial_manifest_cleanup_keeps_unconfirmed_records() {
        let recorded = vec![
            MountSnapshotRef {
                device_identifier: "disk5s1".into(),
                uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
            },
            MountSnapshotRef {
                device_identifier: "disk5s1".into(),
                uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
            },
        ];
        let confirmed_deleted = vec![recorded[0].clone()];
        let current = Vec::new();

        let remaining = manifest_entries_remaining_after_partial_delete(
            &recorded,
            &confirmed_deleted,
            &current,
        );
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].uuid.as_str(), recorded[1].uuid.as_str());

        let current = vec![MountSnapshotRef {
            device_identifier: "DISK5S1".into(),
            uuid: "a8a38394-49ae-42f9-85fb-14dd9ea549FA".into(),
        }];
        let remaining = manifest_entries_remaining_after_partial_delete(
            &recorded,
            &confirmed_deleted,
            &current,
        );
        assert_eq!(remaining.len(), 2);
    }

    #[test]
    fn cleanup_requests_do_not_accept_a_caller_supplied_manifest() {
        let request = MountRequest {
            version: MOUNT_HELPER_PROTOCOL_VERSION,
            request_id: "request-1".into(),
            method: MountMethod::RecordCleanup {
                uid: 501,
                lease_id: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
            },
        };
        let encoded = serde_json::to_string(&request).unwrap();
        assert!(!encoded.contains("snapshot_manifest"));
    }

    #[tokio::test]
    async fn cleanup_requires_an_existing_journal_manifest() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let error = cleanup_request(501, 501, "A8A38394-49AE-42F9-85FB-14DD9EA549FA", &journal)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("existing journal entry"));

        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mounts")
            .fetch_one(&journal)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn status_reports_pending_cleanup_by_volume() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'cleanup_pending', datetime('now'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .bind("snapshot")
        .bind("")
        .bind("[]")
        .execute(&journal)
        .await
        .unwrap();

        let status = status_result(&journal, 501).await.unwrap();
        assert_eq!(status.active_mounts, 1);
        assert_eq!(status.pending_cleanup, 1);
        assert_eq!(
            status.blocked_volume_uuids,
            vec!["C8A38394-49AE-42F9-85FB-14DD9EA549FA"]
        );
    }

    #[tokio::test]
    async fn volume_status_detects_other_uid_without_listing_other_volumes() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, 'cleanup_pending', datetime('now'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(502_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .bind("snapshot")
        .bind("")
        .bind("[]")
        .execute(&journal)
        .await
        .unwrap();

        let status =
            status_for_volume_result(501, 501, "c8a38394-49ae-42f9-85fb-14dd9ea549fa", &journal)
                .await
                .unwrap();
        assert_eq!(status.active_mounts, 1);
        assert_eq!(status.pending_cleanup, 1);
        assert_eq!(
            status.blocked_volume_uuids,
            vec!["C8A38394-49AE-42F9-85FB-14DD9EA549FA"]
        );
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
            build_commit: None,
            active_mounts: 0,
            pending_cleanup: 0,
            blocked_volume_uuids: vec![],
            recovery_leases: vec![],
        };
        assert!(validate_status_result(&result).is_err());
    }

    #[test]
    fn mount_status_accepts_the_unchanged_v098_helper() {
        let result = MountStatusResult {
            helper_version: LEGACY_ROOT_MOUNT_HELPER_VERSION.into(),
            build_commit: None,
            active_mounts: 0,
            pending_cleanup: 0,
            blocked_volume_uuids: vec![],
            recovery_leases: vec![],
        };
        assert!(validate_status_result(&result).is_ok());
    }

    #[test]
    fn mount_status_accepts_the_current_identity_bound_helper() {
        let result = MountStatusResult {
            helper_version: ROOT_MOUNT_HELPER_VERSION.into(),
            build_commit: None,
            active_mounts: 0,
            pending_cleanup: 0,
            blocked_volume_uuids: vec![],
            recovery_leases: vec![],
        };
        assert!(validate_status_result(&result).is_ok());
    }
}
