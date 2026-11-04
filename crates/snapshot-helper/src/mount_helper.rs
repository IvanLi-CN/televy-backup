use std::collections::HashMap;
use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{self, BufRead, Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::thread;
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
    DEFAULT_MOUNT_HELPER_INSTALL_LOCK, DEFAULT_MOUNT_HELPER_JOURNAL,
    DEFAULT_MOUNT_HELPER_MUTATION_LOCK, DEFAULT_MOUNT_HELPER_SOCKET, MOUNT_HELPER_PROTOCOL_VERSION,
    MountLeaseResult, MountLegacyRepairResult, MountMethod, MountRecoveryLease, MountReleaseResult,
    MountRequest, MountResponse, MountResponseResult, MountSnapshotRef, MountStatusResult,
    validate_mount_request,
};

/// Stable v4 wire identity retained by the byte-identical Snapshot Access artifact.
pub const MOUNT_HELPER_WIRE_VERSION: &str = "0.5.0";
pub const ROOT_MOUNT_HELPER_VERSION: &str = "0.5.7";
pub const PREVIOUS_ROOT_MOUNT_HELPER_VERSION: &str = "0.5.4";
pub const PREVIOUS_PREVIOUS_ROOT_MOUNT_HELPER_VERSION: &str = "0.5.3";
pub const HISTORICAL_CLEANUP_ROOT_MOUNT_HELPER_VERSION: &str = "0.5.2";
pub const HISTORICAL_PREVIOUS_ROOT_MOUNT_HELPER_VERSION: &str = "0.5.1";
/// Historical components that can perform journal-bound pending cleanup. Keep
/// CLI and daemon upgrade/cleanup predicates on the same compatibility set.
pub const CLEANUP_COMPATIBLE_ROOT_MOUNT_HELPER_VERSIONS: &[&str] = &[
    "0.5.6",
    "0.5.5",
    PREVIOUS_ROOT_MOUNT_HELPER_VERSION,
    PREVIOUS_PREVIOUS_ROOT_MOUNT_HELPER_VERSION,
    HISTORICAL_CLEANUP_ROOT_MOUNT_HELPER_VERSION,
    HISTORICAL_PREVIOUS_ROOT_MOUNT_HELPER_VERSION,
];
/// Compatibility alias for callers that still name the original component release.
pub const LEGACY_ROOT_MOUNT_HELPER_VERSION: &str = MOUNT_HELPER_WIRE_VERSION;
pub const HISTORICAL_ROOT_MOUNT_HELPER_VERSION: &str = "0.9.8";
const SOCKET_DIRECTORY_MODE: u32 = 0o711;
const MOUNT_ROOT_BASE: &str = "/private/var/tmp/televybackup-snapshot-mounts";
const STATUS_REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const STATUS_COMMAND_TIMEOUT: Duration = Duration::from_secs(4);
const MUTATING_REQUEST_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MAX_COMMAND_OUTPUT_BYTES: usize = 8 * 1024 * 1024;
const COMMAND_POLL_INTERVAL: Duration = Duration::from_millis(25);
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

pub fn configured_install_lock_path() -> PathBuf {
    std::env::var_os("TELEVYBACKUP_SNAPSHOT_MOUNT_INSTALL_LOCK")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_MOUNT_HELPER_INSTALL_LOCK))
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
    acquire_file_lock(
        configured_mutation_lock_path(),
        "snapshot mount mutation lock",
    )
}

/// Serialize privileged helper installation and removal across CLI processes. This lock is
/// separate from the journal mutation lock because the latter must be released before the
/// newly bootstrapped helper can start its own recovery transaction.
pub fn acquire_install_lock() -> Result<MutationFileLock, MountHelperError> {
    acquire_file_lock(
        configured_install_lock_path(),
        "snapshot mount helper installation lock",
    )
}

fn acquire_file_lock(
    path: PathBuf,
    description: &str,
) -> Result<MutationFileLock, MountHelperError> {
    let parent = path
        .parent()
        .ok_or_else(|| MountHelperError::Protocol(format!("{description} has no parent")))?;
    ensure_root_directory(parent)?;
    if let Ok(metadata) = fs::symlink_metadata(&path)
        && (metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err(MountHelperError::Protocol(format!(
            "unsafe root-owned {description}"
        )));
    }
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600))?;
    ensure_root_owned_file(&path, description)?;
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
    if result.helper_version != MOUNT_HELPER_WIRE_VERSION
        && result.helper_version != ROOT_MOUNT_HELPER_VERSION
        && result.helper_version != "0.5.5"
        && result.helper_version != PREVIOUS_ROOT_MOUNT_HELPER_VERSION
        && result.helper_version != PREVIOUS_PREVIOUS_ROOT_MOUNT_HELPER_VERSION
        && result.helper_version != HISTORICAL_CLEANUP_ROOT_MOUNT_HELPER_VERSION
        && result.helper_version != HISTORICAL_PREVIOUS_ROOT_MOUNT_HELPER_VERSION
        && result.helper_version != HISTORICAL_ROOT_MOUNT_HELPER_VERSION
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
        reconcile_inventory: false,
    })?;
    match response.result {
        Some(MountResponseResult::Released(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "release response missing result".into(),
        )),
    }
}

pub fn reconcile_inventory(
    uid: u32,
    lease_id: &str,
) -> Result<MountReleaseResult, MountHelperError> {
    let response = request(MountMethod::Release {
        uid,
        lease_id: lease_id.to_string(),
        reconcile_inventory: true,
    })?;
    match response.result {
        Some(MountResponseResult::Released(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "inventory reconciliation response missing result".into(),
        )),
    }
}

/// Explicitly repair stale rows written before the helper persisted mount device identity.
/// Status remains read-only and ordinary Release remains conservative. This operation is the
/// only path allowed to backfill a missing device identity; the root helper still performs every
/// journal, path, age, volume/device and inventory check while holding its mutation lock.
pub fn repair_legacy(uid: u32) -> Result<MountLegacyRepairResult, MountHelperError> {
    let response = request(MountMethod::RepairLegacy { uid })?;
    match response.result {
        Some(MountResponseResult::LegacyRepaired(result)) => Ok(result),
        _ => Err(MountHelperError::Protocol(
            "legacy repair response missing result".into(),
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
        | MountMethod::RepairLegacy { .. }
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
    let status_lock = Arc::new(AsyncMutex::new(()));
    loop {
        let (stream, _) = listener.accept().await?;
        let journal = journal.clone();
        let mutation_lock = Arc::clone(&mutation_lock);
        let status_lock = Arc::clone(&status_lock);
        tokio::spawn(async move {
            let result = serve_connection(stream, journal, mutation_lock, status_lock).await;
            if let Err(error) = result {
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
        if !matches!(
            state.as_str(),
            "reserved" | "prepared" | "active" | "cleanup_pending"
        ) {
            // Unknown states belong to a newer or corrupted journal schema. Leave the exact row
            // untouched so status can report a blocking unknown record; startup recovery must
            // never mutate an unrecognized state.
            continue;
        }
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
            && !inventory_matches_reservation_baseline(
                &device_identifier,
                &reservation_manifest_json,
            )
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
                    // before the mounted device identity was persisted. Without that identity,
                    // cleanup_mount must never unmount a substituted filesystem. A prepared
                    // unmounted root can proceed only after the complete kernel absence proof.
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
    status_lock: Arc<AsyncMutex<()>>,
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
                        | MountMethod::RepairLegacy { .. }
                        | MountMethod::RecordCleanup { .. }
                        | MountMethod::Cleanup { .. }
                );
                let needs_status_lock = matches!(
                    &request.method,
                    MountMethod::Status | MountMethod::StatusVolume { .. }
                );
                if needs_status_lock {
                    let _status_guard = status_lock.lock().await;
                    handle_request(request, peer_uid, &journal).await
                } else if needs_file_lock {
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
        MountMethod::Release {
            uid,
            lease_id,
            reconcile_inventory,
        } => release_request(peer_uid, uid, &lease_id, reconcile_inventory, journal)
            .await
            .map(MountResponseResult::Released),
        MountMethod::RepairLegacy { uid } => repair_legacy_request(peer_uid, uid, journal)
            .await
            .map(MountResponseResult::LegacyRepaired),
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
    let classification =
        classify_status_rows(pool, (peer_uid != 0).then_some(peer_uid), None).await?;
    Ok(MountStatusResult {
        // Keep the wire compatibility version stable for the approved Snapshot Access
        // artifact. The additive component_version carries the implementation bump.
        helper_version: MOUNT_HELPER_WIRE_VERSION.to_string(),
        component_version: Some(ROOT_MOUNT_HELPER_VERSION.to_string()),
        build_commit: option_env!("TELEVYBACKUP_BUILD_COMMIT").map(str::to_owned),
        active_mounts: classification.active_mounts,
        pending_cleanup: Some(classification.pending_cleanup),
        blocked_volume_uuids: classification.blocked_volume_uuids,
        recovery_leases: classification.recovery_leases,
        pending_cleanup_leases: classification.pending_cleanup_leases,
        live_mounts: Some(classification.live_mounts),
        recoverable_stale_mounts: Some(classification.recoverable_stale_mounts),
        unknown_blocking_mounts: Some(classification.unknown_blocking_mounts),
        inventory_drift_mounts: Some(classification.inventory_drift_mounts),
        inventory_drift_leases: classification.inventory_drift_leases,
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
    let classification = classify_status_rows(pool, None, Some((volume_uuid, uid))).await?;
    Ok(MountStatusResult {
        helper_version: MOUNT_HELPER_WIRE_VERSION.to_string(),
        component_version: Some(ROOT_MOUNT_HELPER_VERSION.to_string()),
        build_commit: option_env!("TELEVYBACKUP_BUILD_COMMIT").map(str::to_owned),
        active_mounts: classification.active_mounts,
        pending_cleanup: Some(classification.pending_cleanup),
        blocked_volume_uuids: classification.blocked_volume_uuids,
        recovery_leases: classification.recovery_leases,
        pending_cleanup_leases: classification.pending_cleanup_leases,
        live_mounts: Some(classification.live_mounts),
        recoverable_stale_mounts: Some(classification.recoverable_stale_mounts),
        unknown_blocking_mounts: Some(classification.unknown_blocking_mounts),
        inventory_drift_mounts: Some(classification.inventory_drift_mounts),
        inventory_drift_leases: classification.inventory_drift_leases,
    })
}

struct StatusClassification {
    active_mounts: u32,
    pending_cleanup: u32,
    live_mounts: u32,
    recoverable_stale_mounts: u32,
    unknown_blocking_mounts: u32,
    inventory_drift_mounts: u32,
    recovery_leases: Vec<MountRecoveryLease>,
    pending_cleanup_leases: Vec<MountRecoveryLease>,
    inventory_drift_leases: Vec<MountRecoveryLease>,
    blocked_volume_uuids: Vec<String>,
}

async fn classify_status_rows(
    pool: &SqlitePool,
    scope_uid: Option<u32>,
    volume_scope: Option<(&str, u32)>,
) -> Result<StatusClassification, MountHelperError> {
    classify_status_rows_with_observation(
        pool,
        scope_uid,
        volume_scope,
        &SystemLegacyRepairObservation,
    )
    .await
}

async fn classify_status_rows_with_observation(
    pool: &SqlitePool,
    scope_uid: Option<u32>,
    volume_scope: Option<(&str, u32)>,
    observation: &impl LegacyRepairObservation,
) -> Result<StatusClassification, MountHelperError> {
    let mut statement = String::from(
        "SELECT lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_device, state, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, CASE WHEN last_renewed_at <= datetime('now', ?) THEN 1 ELSE 0 END AS stale_row, CASE WHEN last_renewed_at > datetime('now', ?) THEN 1 ELSE 0 END AS live_row FROM mounts WHERE state IS NOT 'released'",
    );
    if scope_uid.is_some() {
        statement.push_str(" AND uid = ?");
    }
    if volume_scope.is_some() {
        statement.push_str(" AND lower(volume_uuid) = lower(?)");
    }
    let mut query = sqlx::query(&statement)
        .bind(RECOVERY_AGE_SECONDS)
        .bind(RECOVERY_AGE_SECONDS);
    if let Some(scope_uid) = scope_uid {
        query = query.bind(i64::from(scope_uid));
    }
    if let Some((volume_uuid, _)) = volume_scope {
        query = query.bind(volume_uuid);
    }
    let unfinished_rows = query.fetch_all(pool).await?;
    let mut classification = StatusClassification {
        active_mounts: unfinished_rows.len() as u32,
        pending_cleanup: 0,
        live_mounts: 0,
        recoverable_stale_mounts: 0,
        unknown_blocking_mounts: 0,
        inventory_drift_mounts: 0,
        recovery_leases: Vec::new(),
        pending_cleanup_leases: Vec::new(),
        inventory_drift_leases: Vec::new(),
        blocked_volume_uuids: Vec::new(),
    };
    let mut inventory_cache: HashMap<String, Option<Vec<MountSnapshotRef>>> = HashMap::new();
    // A status response is a read-only observation. Audit each distinct binding
    // once, rather than spawning diskutil once per historical tombstone and
    // exhausting the bounded status IPC deadline. Never reuse across requests
    // or in a mutating Release operation.
    let mut binding_cache = HashMap::new();
    for row in &unfinished_rows {
        let lease_id: String = row.try_get("lease_id")?;
        let uid: i64 = row.try_get("uid")?;
        let volume_uuid: String = row.try_get("volume_uuid")?;
        let device_identifier: String = row.try_get("device_identifier")?;
        let mount_device: i64 = row.try_get("mount_device").unwrap_or_default();
        let state: String = row.try_get("state")?;
        let mount_root: String = row.try_get("mount_root")?;
        let snapshot_manifest: String = row.try_get("snapshot_manifest")?;
        let reservation_manifest: String = row.try_get("reservation_manifest")?;
        let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
        let stale = row.try_get::<i64, _>("stale_row")? == 1;
        let live = row.try_get::<i64, _>("live_row")? == 1;
        let volume_is_unblocked =
            !volume_uuid.is_empty() && !classification.blocked_volume_uuids.contains(&volume_uuid);
        if volume_is_unblocked {
            let blocked_volume_uuids = &mut classification.blocked_volume_uuids;
            blocked_volume_uuids.push(volume_uuid.clone());
        }
        let manifest_valid = parse_journal_manifest(&snapshot_manifest).is_ok();
        let reservation_valid = reservation_manifest_valid == 1
            && parse_reservation_manifest(&reservation_manifest).is_ok();
        let managed_root = expected_mount_root(uid, &lease_id)
            .ok()
            .as_deref()
            .is_some_and(|expected| !mount_root.is_empty() && Path::new(&mount_root) == expected);
        let journal_identity_valid = uid > 0
            && validate_lease_id(&lease_id).is_ok()
            && validate_uuid(&volume_uuid).is_ok()
            && validate_device(&device_identifier).is_ok()
            && reservation_valid
            && ((mount_root.is_empty() && (snapshot_manifest == "[]" || manifest_valid))
                || (!mount_root.is_empty() && mount_device > 0 && manifest_valid && managed_root));
        let recoverable = state != "cleanup_pending"
            && stale
            && volume_scope
                .map(|(_, owner_uid)| uid == i64::from(owner_uid))
                .or_else(|| scope_uid.map(|scope_uid| uid == i64::from(scope_uid)))
                .unwrap_or(true)
            && journal_identity_valid
            && matches!(state.as_str(), "reserved" | "prepared" | "active");
        let owner_matches = volume_scope
            .map(|(_, owner_uid)| uid == i64::from(owner_uid))
            .unwrap_or(true);
        let pending_unmounted_identity = mount_device == 0
            && managed_root
            && manifest_valid
            && reservation_valid
            && uid > 0
            && validate_lease_id(&lease_id).is_ok()
            && validate_uuid(&volume_uuid).is_ok()
            && validate_device(&device_identifier).is_ok();
        let pending_cleanup_candidate =
            state == "cleanup_pending" && (journal_identity_valid || pending_unmounted_identity);
        let known_state = matches!(
            state.as_str(),
            "reserved" | "prepared" | "active" | "cleanup_pending"
        );
        if !known_state {
            classification.unknown_blocking_mounts += 1;
        } else if pending_cleanup_candidate {
            classification.pending_cleanup += 1;
            if owner_matches {
                classification
                    .pending_cleanup_leases
                    .push(MountRecoveryLease { lease_id, state });
            }
        } else if recoverable {
            classification.recoverable_stale_mounts += 1;
            classification
                .recovery_leases
                .push(MountRecoveryLease { lease_id, state });
        } else if live {
            classification.live_mounts += 1;
        } else {
            classification.unknown_blocking_mounts += 1;
        }
    }

    let mut released_statement = String::from(
        "SELECT lease_id, uid, volume_uuid, device_identifier, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid FROM mounts WHERE state = 'released'",
    );
    if scope_uid.is_some() {
        released_statement.push_str(" AND uid = ?");
    }
    if let Some((_, _)) = volume_scope {
        released_statement.push_str(" AND lower(volume_uuid) = lower(?)");
    }
    let mut released_query = sqlx::query(&released_statement);
    if let Some(scope_uid) = scope_uid {
        released_query = released_query.bind(i64::from(scope_uid));
    }
    if let Some((volume_uuid, _)) = volume_scope {
        released_query = released_query.bind(volume_uuid);
    }
    for row in released_query.fetch_all(pool).await? {
        let lease_id: String = row.try_get("lease_id")?;
        let uid: i64 = row.try_get("uid")?;
        let volume_uuid: String = row.try_get("volume_uuid")?;
        let device_identifier: String = row.try_get("device_identifier")?;
        let mount_root: String = row.try_get("mount_root")?;
        let snapshot_manifest: String = row.try_get("snapshot_manifest")?;
        let reservation_manifest: String = row.try_get("reservation_manifest")?;
        let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
        let baseline = parse_reservation_manifest(&reservation_manifest);
        let identity_valid = uid > 0
            && validate_lease_id(&lease_id).is_ok()
            && validate_uuid(&volume_uuid).is_ok()
            && validate_device(&device_identifier).is_ok()
            && mount_root.is_empty()
            && snapshot_manifest == "[]"
            && reservation_manifest_valid == 1
            && baseline.is_ok()
            && *binding_cache
                .entry((device_identifier.clone(), volume_uuid.clone()))
                .or_insert_with(|| observation.binding_valid(&device_identifier, &volume_uuid));
        let owner_matches = volume_scope
            .map(|(_, owner_uid)| uid == i64::from(owner_uid))
            .unwrap_or(true);
        if !identity_valid || !owner_matches {
            classification.active_mounts += 1;
            classification.unknown_blocking_mounts += 1;
            if !volume_uuid.is_empty()
                && !classification.blocked_volume_uuids.contains(&volume_uuid)
            {
                classification.blocked_volume_uuids.push(volume_uuid);
            }
            continue;
        }
        let baseline = baseline.expect("validated reservation manifest");
        let current_manifest = inventory_cache
            .entry(device_identifier.clone())
            .or_insert_with(|| observation.inventory(&device_identifier).ok())
            .clone();
        match current_manifest {
            Some(current_manifest)
                if manifests_equal(&current_manifest, &baseline)
                    || inventory_change_has_recorded_owner(
                        &current_manifest,
                        &baseline,
                        uid,
                        &volume_uuid,
                        &device_identifier,
                        &unfinished_rows,
                    ) => {}
            Some(_) => {
                classification.active_mounts += 1;
                classification.inventory_drift_mounts += 1;
                if !volume_uuid.is_empty()
                    && !classification.blocked_volume_uuids.contains(&volume_uuid)
                {
                    classification
                        .blocked_volume_uuids
                        .push(volume_uuid.clone());
                }
                classification
                    .inventory_drift_leases
                    .push(MountRecoveryLease {
                        lease_id,
                        state: "released_inventory_drift".to_string(),
                    });
            }
            None => {
                classification.active_mounts += 1;
                classification.unknown_blocking_mounts += 1;
                if !volume_uuid.is_empty()
                    && !classification.blocked_volume_uuids.contains(&volume_uuid)
                {
                    classification.blocked_volume_uuids.push(volume_uuid);
                }
            }
        }
    }
    Ok(classification)
}

/// A released tombstone must not count an exactly journal-owned in-flight snapshot as
/// unrelated drift. Its owning row remains blocking; no baseline or journal row is changed.
fn inventory_change_has_recorded_owner(
    current: &[MountSnapshotRef],
    baseline: &[MountSnapshotRef],
    uid: i64,
    volume: &str,
    device: &str,
    rows: &[sqlx::sqlite::SqliteRow],
) -> bool {
    let mut owners = rows.iter().filter(|row| {
        row.get::<String, _>("volume_uuid")
            .eq_ignore_ascii_case(volume)
    });
    let Some(row) = owners.next() else {
        return false;
    };
    if owners.next().is_some() {
        return false;
    }
    let lease: String = row.get("lease_id");
    let root: String = row.get("mount_root");
    let name: String = row.get("snapshot_name");
    let state: String = row.get("state");
    let owned = parse_reservation_manifest(&row.get::<String, _>("snapshot_manifest"));
    let owner_baseline = parse_reservation_manifest(&row.get::<String, _>("reservation_manifest"));
    if row.get::<i64, _>("uid") != uid
        || uid <= 0
        || row.get::<String, _>("device_identifier") != device
        || !matches!(state.as_str(), "prepared" | "active" | "cleanup_pending")
        || validate_snapshot_name(&name).is_err()
        || row.get::<i64, _>("reservation_manifest_valid") != 1
        || !expected_mount_root(uid, &lease).is_ok_and(|expected| expected == Path::new(&root))
    {
        return false;
    }
    let (Ok(owned), Ok(owner_baseline)) = (owned, owner_baseline) else {
        return false;
    };
    if owned.is_empty()
        || owned.iter().any(|entry| entry.device_identifier != device)
        || !manifests_equal(baseline, &owner_baseline)
    {
        return false;
    }
    let mut expected = baseline.to_vec();
    expected.extend(owned);
    manifests_equal(current, &expected)
}

async fn audit_released_tombstones(
    journal: &SqlitePool,
    volume_uuid: &str,
    device_identifier: &str,
) -> Result<(), MountHelperError> {
    let rows = sqlx::query(
        "SELECT lease_id, uid, volume_uuid, device_identifier, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid FROM mounts WHERE lower(volume_uuid) = lower(?) AND state = 'released'",
    )
    .bind(volume_uuid)
    .fetch_all(journal)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }
    validate_device_volume_binding(device_identifier, volume_uuid)?;
    let current_manifest = snapshot_inventory_manifest(device_identifier)?;
    for row in rows {
        let lease_id: String = row.try_get("lease_id")?;
        let uid: i64 = row.try_get("uid")?;
        let stored_volume_uuid: String = row.try_get("volume_uuid")?;
        let stored_device: String = row.try_get("device_identifier")?;
        let mount_root: String = row.try_get("mount_root")?;
        let snapshot_manifest: String = row.try_get("snapshot_manifest")?;
        let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
        let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
        let baseline = parse_reservation_manifest(&reservation_manifest_json);
        let identity_valid = uid > 0
            && validate_lease_id(&lease_id).is_ok()
            && stored_volume_uuid.eq_ignore_ascii_case(volume_uuid)
            && validate_uuid(&stored_volume_uuid).is_ok()
            && validate_device(&stored_device).is_ok()
            && stored_device == device_identifier
            && mount_root.is_empty()
            && snapshot_manifest == "[]"
            && reservation_manifest_valid == 1;
        let matches = identity_valid
            && stored_device.eq_ignore_ascii_case(device_identifier)
            && baseline
                .as_ref()
                .is_ok_and(|baseline| manifests_equal(&current_manifest, baseline));
        if !matches {
            return Err(MountHelperError::Protocol(
                "released snapshot tombstone inventory changed; explicit reconciliation required"
                    .into(),
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
        "SELECT COUNT(*) FROM mounts WHERE lower(volume_uuid) = lower(?) AND state IS NOT 'released'",
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

/// Repair the legacy journal layout that did not persist the mounted device identity or reservation
/// baseline. Every row remains age-, owner-, path-, volume-identity-, and inventory-gated. A
/// historical device node is rebound through the journaled volume UUID before any Release call.
/// A mounted legacy root is usable only after `statfs(2)` proves the exact recorded APFS snapshot
/// source; a different or unobservable mount remains blocked. When the old reservation baseline is
/// absent, the current inventory is split into the journaled snapshots and the untouched baseline
/// before the ordinary journal-bound Release path is reused. This preserves unrecorded snapshots
/// and makes the action converge instead of repeatedly returning the same aggregate blocker.
trait LegacyRepairObservation {
    fn binding_valid(&self, device: &str, volume: &str) -> bool;
    fn resolve_device(&self, volume: &str, historical: &str) -> Result<String, MountHelperError>;
    fn inventory(&self, device: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError>;
    fn historical_root_unmounted(
        &self,
        root: &Path,
        uid: u32,
        lease: &str,
        name: &str,
        device: &str,
    ) -> Result<bool, MountHelperError>;
}

struct SystemLegacyRepairObservation;

impl LegacyRepairObservation for SystemLegacyRepairObservation {
    fn binding_valid(&self, device: &str, volume: &str) -> bool {
        validate_device_volume_binding(device, volume).is_ok()
    }
    fn resolve_device(&self, volume: &str, historical: &str) -> Result<String, MountHelperError> {
        resolve_legacy_volume_device(volume, historical)
    }

    fn inventory(&self, device: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
        snapshot_inventory_manifest(device)
    }

    fn historical_root_unmounted(
        &self,
        root: &Path,
        uid: u32,
        lease: &str,
        name: &str,
        device: &str,
    ) -> Result<bool, MountHelperError> {
        if !historical_root_metadata_safe(root, uid, lease) {
            return Ok(false);
        }
        Ok(!mounted_legacy_source_or_root(
            &system_mount_table()?,
            root,
            name,
            device,
        ))
    }
}

async fn repair_legacy_request(
    peer_uid: u32,
    uid: u32,
    journal: &SqlitePool,
) -> Result<MountLegacyRepairResult, MountHelperError> {
    repair_legacy_with_observation(peer_uid, uid, journal, &SystemLegacyRepairObservation).await
}

async fn repair_legacy_with_observation(
    peer_uid: u32,
    uid: u32,
    journal: &SqlitePool,
    observation: &impl LegacyRepairObservation,
) -> Result<MountLegacyRepairResult, MountHelperError> {
    if peer_uid != uid || uid == 0 {
        return Err(MountHelperError::Protocol(
            "legacy repair UID does not match peer".into(),
        ));
    }
    let rows = sqlx::query(
        "SELECT lease_id, volume_uuid, device_identifier, snapshot_name, mount_root, mount_device, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, last_renewed_at FROM mounts WHERE uid = ? AND (state IS NOT 'released' OR mount_root != '' OR snapshot_manifest != '[]' OR reservation_manifest_valid != 1) AND last_renewed_at <= datetime('now', ?)",
    )
    .bind(i64::from(uid))
    .bind(RECOVERY_AGE_SECONDS)
    .fetch_all(journal)
    .await?;
    let mut result = MountLegacyRepairResult {
        audited: rows.len() as u32,
        ..Default::default()
    };
    // APFS device nodes are not stable across reboot or volume reattachment. Resolve each
    // journaled volume once from its UUID instead of trusting a historical disk node.
    let mut device_cache: HashMap<String, Option<String>> = HashMap::new();
    let mut inventory_cache: HashMap<String, Option<Vec<MountSnapshotRef>>> = HashMap::new();
    for row in rows {
        let lease_id: String = row.try_get("lease_id")?;
        let volume_uuid: String = row.try_get("volume_uuid")?;
        let device_identifier: String = row.try_get("device_identifier")?;
        let snapshot_name: String = row.try_get("snapshot_name")?;
        let mount_root: String = row.try_get("mount_root")?;
        let mount_device: i64 = row.try_get("mount_device").unwrap_or_default();
        let snapshot_manifest_json: String = row.try_get("snapshot_manifest")?;
        let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
        let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
        let state: String = row.try_get("state")?;
        let eligible = matches!(
            state.as_str(),
            "reserved" | "prepared" | "active" | "cleanup_pending" | "released"
        ) && validate_lease_id(&lease_id).is_ok()
            && validate_uuid(&volume_uuid).is_ok();
        if !eligible {
            result.blocked += 1;
            continue;
        }

        let resolved_device = if let Some(cached) = device_cache.get(&volume_uuid) {
            cached.clone()
        } else {
            let resolved = observation
                .resolve_device(&volume_uuid, &device_identifier)
                .ok();
            device_cache.insert(volume_uuid.clone(), resolved.clone());
            resolved
        };
        let Some(resolved_device) = resolved_device else {
            result.blocked += 1;
            continue;
        };

        let manifest = if snapshot_manifest_json == "[]" {
            Vec::new()
        } else {
            match parse_journal_manifest(&snapshot_manifest_json) {
                Ok(manifest) => manifest,
                Err(_) => {
                    result.blocked += 1;
                    continue;
                }
            }
        };
        // Old helpers accepted private caller-owned data directories. Never pass those
        // paths to privileged cleanup. A terminal, unmounted row whose snapshots are
        // already absent can instead be retired by a journal-only compare-and-swap.
        if state == "released"
            && mount_device == 0
            && !manifest.is_empty()
            && !snapshot_name.is_empty()
            && historical_root_layout(Path::new(&mount_root), uid, &lease_id)
        {
            let retired = retire_historical_released_row(
                journal,
                uid,
                &lease_id,
                &volume_uuid,
                &device_identifier,
                &resolved_device,
                &snapshot_name,
                &mount_root,
                &snapshot_manifest_json,
                &reservation_manifest_json,
                reservation_manifest_valid,
                &manifest,
                observation,
            )
            .await
            .unwrap_or(false);
            if retired {
                result.repaired += 1;
                result.released += 1;
                if reservation_manifest_valid == 0 {
                    result.migrated += 1;
                }
            } else {
                result.blocked += 1;
            }
            continue;
        }
        let expected_root = if mount_root.is_empty() {
            None
        } else {
            let expected = match expected_mount_root(i64::from(uid), &lease_id) {
                Ok(expected) => expected,
                Err(_) => {
                    result.blocked += 1;
                    continue;
                }
            };
            if Path::new(&mount_root) != expected || !validate_legacy_mount_root(&expected) {
                result.blocked += 1;
                continue;
            }
            Some(expected)
        };

        // A missing or ordinary directory at the recorded root is a provably unmounted legacy
        // root; it does not need a device number to be cleaned. A different filesystem at the
        // root is only usable after statfs proves the exact APFS snapshot source. Any inability
        // to make that distinction remains fail-closed.
        let expected_device = match expected_root.as_deref() {
            None => None,
            Some(root) => {
                match legacy_mounted_snapshot_device(root, &snapshot_name, &resolved_device) {
                    Ok(device) => device,
                    Err(_) => {
                        result.blocked += 1;
                        continue;
                    }
                }
            }
        };

        let stored_baseline = match legacy_reservation_baseline(
            &reservation_manifest_json,
            reservation_manifest_valid,
        ) {
            Ok(baseline) => baseline,
            Err(_) => {
                result.blocked += 1;
                continue;
            }
        };
        let current_manifest = if let Some(cached) = inventory_cache.get(&resolved_device) {
            cached.clone()
        } else {
            let current = observation.inventory(&resolved_device).ok();
            inventory_cache.insert(resolved_device.clone(), current.clone());
            current
        };
        let Some(current_manifest) = current_manifest else {
            result.blocked += 1;
            continue;
        };
        // Match by snapshot UUID rather than the historical device node. The current inventory
        // has already been queried from the UUID-bound APFS volume, so this safely handles disk
        // identifiers that changed after reboot while retaining unrecorded snapshots as baseline.
        let Some((reservation_manifest, release_manifest)) =
            partition_legacy_manifests(&current_manifest, stored_baseline.as_deref(), &manifest)
        else {
            result.blocked += 1;
            continue;
        };
        if stored_baseline.is_none() {
            result.migrated += 1;
        }
        let reservation_manifest_json_new = match serde_json::to_string(&reservation_manifest) {
            Ok(json) => json,
            Err(_) => {
                result.blocked += 1;
                continue;
            }
        };
        let release_manifest_json = match serde_json::to_string(&release_manifest) {
            Ok(json) => json,
            Err(_) => {
                result.blocked += 1;
                continue;
            }
        };

        // Old helpers marked rows released without clearing their mount/manifest fields.
        // Only after the same identity and inventory checks as other legacy rows may
        // they re-enter cleanup. Release otherwise treats released rows as terminal.
        let repair_state = if state == "released" {
            "cleanup_pending"
        } else {
            state.as_str()
        };
        let updated = sqlx::query(
            "UPDATE mounts SET device_identifier = ?, mount_device = ?, snapshot_manifest = ?, reservation_manifest = ?, reservation_manifest_valid = 1, state = ? WHERE lease_id = ? AND uid = ? AND state = ? AND mount_device = ? AND mount_root = ? AND snapshot_manifest = ?",
        )
        .bind(&resolved_device)
        .bind(expected_device.map(|device| i64::try_from(device).unwrap_or(i64::MAX)).unwrap_or(0))
        .bind(&release_manifest_json)
        .bind(&reservation_manifest_json_new)
        .bind(repair_state)
        .bind(&lease_id)
        .bind(i64::from(uid))
        .bind(&state)
        .bind(mount_device)
        .bind(&mount_root)
        .bind(&snapshot_manifest_json)
        .execute(journal)
        .await?;
        if updated.rows_affected() != 1 {
            result.blocked += 1;
            continue;
        }

        if release_manifest.is_empty() {
            let cleanup_state = if let Some(root) = expected_root.as_deref() {
                if expected_device.is_some() {
                    cleanup_mount(
                        root,
                        &[],
                        expected_device,
                        &volume_uuid,
                        journal,
                        &lease_id,
                        &resolved_device,
                        &reservation_manifest_json_new,
                        "[]",
                    )
                    .await
                } else {
                    cleanup_legacy_unmounted_root(
                        root,
                        &[],
                        &volume_uuid,
                        journal,
                        &lease_id,
                        &resolved_device,
                        &reservation_manifest_json_new,
                        "[]",
                    )
                    .await
                }
            } else {
                record_released_tombstone(journal, &lease_id, &reservation_manifest_json_new, "[]")
                    .await
            };
            update_state(journal, &lease_id, cleanup_state).await?;
            result.repaired += 1;
            if cleanup_state == "released" {
                result.released += 1;
            }
            continue;
        }

        if let Some(root) = expected_root.as_deref()
            && expected_device.is_none()
        {
            let cleanup_state = cleanup_legacy_unmounted_root(
                root,
                &release_manifest,
                &volume_uuid,
                journal,
                &lease_id,
                &resolved_device,
                &reservation_manifest_json_new,
                &release_manifest_json,
            )
            .await;
            update_state(journal, &lease_id, cleanup_state).await?;
            result.repaired += 1;
            if cleanup_state == "released" {
                result.released += 1;
            }
            continue;
        }

        match release_request(peer_uid, uid, &lease_id, false, journal).await {
            Ok(release) => {
                result.repaired += 1;
                if release.cleanup_state == "released" {
                    result.released += 1;
                }
            }
            Err(_) => {
                // Keep a migrated reservation baseline so a failed deletion can be retried as a
                // normal pending-cleanup row. A mount-device backfill alone is never retained.
                let _ = sqlx::query(
                    "UPDATE mounts SET device_identifier = ?, mount_device = 0 WHERE lease_id = ? AND uid = ? AND mount_device = ? AND state = ? AND snapshot_manifest = ?",
                )
                .bind(&resolved_device)
                .bind(&lease_id)
                .bind(i64::from(uid))
                .bind(expected_device.map(|device| i64::try_from(device).unwrap_or(i64::MAX)).unwrap_or(0))
                .bind(repair_state)
                .bind(&release_manifest_json)
                .execute(journal)
                .await;
            }
        }
    }
    let status =
        classify_status_rows_with_observation(journal, Some(uid), None, observation).await?;
    result.remaining = status.active_mounts;
    result.unknown = status.unknown_blocking_mounts;
    Ok(result)
}

/// This path only retires an obsolete journal identity. It must never authorize
/// filesystem cleanup in a caller-owned historical data directory.
#[allow(clippy::too_many_arguments)]
async fn retire_historical_released_row(
    journal: &SqlitePool,
    uid: u32,
    lease: &str,
    volume: &str,
    historical_device: &str,
    device: &str,
    snapshot_name: &str,
    root: &str,
    manifest_json: &str,
    baseline_json: &str,
    baseline_valid: i64,
    manifest: &[MountSnapshotRef],
    observation: &impl LegacyRepairObservation,
) -> Result<bool, MountHelperError> {
    let Some(retained) = verified_historical_retirement_baseline(
        uid,
        lease,
        volume,
        historical_device,
        device,
        snapshot_name,
        root,
        baseline_json,
        baseline_valid,
        manifest,
        observation,
    )?
    else {
        return Ok(false);
    };
    let retained_json = serde_json::to_string(&retained)
        .map_err(|_| MountHelperError::Protocol("cannot encode retained inventory".into()))?;
    let updated = sqlx::query(
        "UPDATE mounts SET mount_root = '', mount_device = 0, snapshot_manifest = '[]', reservation_manifest = ?, reservation_manifest_valid = 1, device_identifier = ? WHERE lease_id = ? AND uid = ? AND state = 'released' AND mount_device = 0 AND mount_root = ? AND snapshot_name = ? AND volume_uuid = ? AND device_identifier = ? AND snapshot_manifest = ? AND reservation_manifest = ? AND reservation_manifest_valid = ? AND last_renewed_at <= datetime('now', '-300 seconds') AND NOT EXISTS (SELECT 1 FROM mounts AS live WHERE lower(live.volume_uuid) = lower(mounts.volume_uuid) AND live.state IS NOT 'released')",
    )
    .bind(retained_json)
    .bind(device)
    .bind(lease)
    .bind(i64::from(uid))
    .bind(root)
    .bind(snapshot_name)
    .bind(volume)
    .bind(historical_device)
    .bind(manifest_json)
    .bind(baseline_json)
    .bind(baseline_valid)
    .execute(journal)
    .await?;
    Ok(updated.rows_affected() == 1)
}

#[allow(clippy::too_many_arguments)]
fn verified_historical_retirement_baseline(
    uid: u32,
    lease: &str,
    volume: &str,
    historical_device: &str,
    device: &str,
    snapshot_name: &str,
    root: &str,
    baseline_json: &str,
    baseline_valid: i64,
    manifest: &[MountSnapshotRef],
    observation: &impl LegacyRepairObservation,
) -> Result<Option<Vec<MountSnapshotRef>>, MountHelperError> {
    if uid == 0
        || manifest.is_empty()
        || validate_snapshot_name(snapshot_name).is_err()
        || validate_uuid(volume).is_err()
        || validate_device(device).is_err()
        || !historical_root_layout(Path::new(root), uid, lease)
    {
        return Ok(None);
    }
    let baseline = legacy_reservation_baseline(baseline_json, baseline_valid)?;
    let current = observation.inventory(device)?;
    let Some((retained, to_delete)) =
        partition_legacy_manifests(&current, baseline.as_deref(), manifest)
    else {
        return Ok(None);
    };
    if !to_delete.is_empty()
        || !observation.historical_root_unmounted(
            Path::new(root),
            uid,
            lease,
            snapshot_name,
            device,
        )?
        || observation.resolve_device(volume, historical_device)? != device
        || !manifests_equal(&current, &observation.inventory(device)?)
        || !observation.historical_root_unmounted(
            Path::new(root),
            uid,
            lease,
            snapshot_name,
            device,
        )?
    {
        return Ok(None);
    }
    Ok(Some(retained))
}

fn historical_root_layout(root: &Path, uid: u32, lease: &str) -> bool {
    root.is_absolute()
        && root.components().all(|component| {
            matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
        && validate_lease_id(lease).is_ok()
        && root.ends_with(
            Path::new("snapshot-access")
                .join("mounts")
                .join(uid.to_string())
                .join(lease),
        )
        && !root.as_os_str().as_bytes().contains(&0)
}

fn historical_root_metadata_safe(root: &Path, uid: u32, lease: &str) -> bool {
    if !historical_root_layout(root, uid, lease) {
        return false;
    }
    root.ancestors()
        .all(|ancestor| match fs::symlink_metadata(ancestor) {
            Ok(metadata) => {
                metadata.is_dir()
                    && !metadata.file_type().is_symlink()
                    && (metadata.uid() == 0 || metadata.uid() == uid)
                    && metadata.mode() & 0o022 == 0
            }
            Err(error) => error.kind() == std::io::ErrorKind::NotFound,
        })
}

struct ObservedMount {
    source: String,
    root: PathBuf,
}

fn normalized_mount_path(path: &Path) -> PathBuf {
    for (prefix, replacement) in [
        ("/System/Volumes/Data/Users", "/Users"),
        ("/System/Volumes/Data/private/var", "/private/var"),
        ("/var", "/private/var"),
    ] {
        if let Ok(suffix) = path.strip_prefix(prefix) {
            return Path::new(replacement).join(suffix);
        }
    }
    path.to_path_buf()
}

fn mounted_legacy_source_or_root(
    mounts: &[ObservedMount],
    root: &Path,
    name: &str,
    device: &str,
) -> bool {
    let source = format!("{name}@/dev/{device}");
    let root = normalized_mount_path(root);
    mounts.iter().any(|mount| {
        mount.source == source || normalized_mount_path(&mount.root).starts_with(&root)
    })
}

#[cfg(target_os = "macos")]
fn system_mount_table() -> Result<Vec<ObservedMount>, MountHelperError> {
    for _ in 0..3 {
        let count = unsafe { libc::getfsstat(std::ptr::null_mut(), 0, libc::MNT_NOWAIT) };
        if count <= 0 || count > 4096 {
            break;
        }
        let capacity = count as usize + 16;
        let mut entries = Vec::with_capacity(capacity);
        entries.resize_with(capacity, std::mem::MaybeUninit::<libc::statfs>::uninit);
        let filled = unsafe {
            libc::getfsstat(
                entries.as_mut_ptr().cast(),
                (capacity * std::mem::size_of::<libc::statfs>()) as i32,
                libc::MNT_NOWAIT,
            )
        };
        if filled <= 0 {
            break;
        }
        if filled as usize >= capacity {
            continue;
        }
        let decode = |bytes: &[libc::c_char]| -> Result<String, MountHelperError> {
            let bytes: Vec<u8> = bytes
                .iter()
                .take_while(|byte| **byte != 0)
                .map(|byte| *byte as u8)
                .collect();
            String::from_utf8(bytes).map_err(|_| {
                MountHelperError::Protocol("cannot decode kernel mount identity".into())
            })
        };
        return entries
            .iter()
            .take(filled as usize)
            .map(|entry| {
                let entry = unsafe { entry.assume_init_ref() };
                Ok(ObservedMount {
                    source: decode(&entry.f_mntfromname)?,
                    root: PathBuf::from(decode(&entry.f_mntonname)?),
                })
            })
            .collect();
    }
    Err(MountHelperError::Protocol(
        "cannot observe a complete kernel mount table".into(),
    ))
}

#[cfg(not(target_os = "macos"))]
fn system_mount_table() -> Result<Vec<ObservedMount>, MountHelperError> {
    Err(MountHelperError::Protocol(
        "historical mount retirement requires macOS".into(),
    ))
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

fn created_snapshot_name(output: &[u8]) -> Result<String, MountHelperError> {
    let text = std::str::from_utf8(output)
        .map_err(|_| MountHelperError::Protocol("invalid snapshot creation receipt".into()))?;
    let mut dates = text.lines().filter_map(|line| {
        line.trim()
            .strip_prefix("Created local snapshot with date:")
            .map(str::trim)
    });
    let date = dates
        .next()
        .ok_or_else(|| MountHelperError::Protocol("missing snapshot creation receipt".into()))?;
    let valid = date.len() == 17
        && date.bytes().enumerate().all(|(index, value)| {
            if [4, 7, 10].contains(&index) {
                value == b'-'
            } else {
                value.is_ascii_digit()
            }
        });
    if !valid || dates.next().is_some() {
        return Err(MountHelperError::Protocol(
            "ambiguous snapshot creation receipt".into(),
        ));
    }
    Ok(format!("com.apple.TimeMachine.{date}.local"))
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
    let creation_output = run_command_output(
        "snapshot_create",
        "tmutil.localsnapshot",
        "/usr/bin/tmutil",
        ["localsnapshot"],
    )?;
    let created_name = created_snapshot_name(&creation_output)?;
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
    validate_snapshot_binding(&device_identifier, &manifest[0].uuid, &created_name)?;
    let confirmed_manifest = snapshot_inventory_manifest(&device_identifier)?;
    if !manifests_equal(&confirmed_manifest, &current_manifest) {
        let _ = update_state(journal, lease_id, "cleanup_pending").await;
        return Err(MountHelperError::Protocol(
            "snapshot inventory changed while recording cleanup".into(),
        ));
    }
    // tmutil may prune older Time Machine snapshots while creating the requested snapshot.
    // Retain every currently existing non-owned snapshot; absent historical snapshots are not
    // cleanup targets. Only the single new UUID remains owned by this reservation.
    let (retained_baseline, present_owned) =
        partition_legacy_manifests(&current_manifest, Some(&reservation_manifest), &manifest)
            .ok_or_else(|| {
                MountHelperError::Protocol("ambiguous snapshot creation inventory".into())
            })?;
    if !manifests_equal(&present_owned, &manifest) {
        return Err(MountHelperError::Protocol(
            "created snapshot disappeared before recording".into(),
        ));
    }
    let retained_json = serde_json::to_string(&retained_baseline)?;
    let manifest_json = serde_json::to_string(&manifest)?;
    persist_prepared_creation(
        journal,
        uid,
        lease_id,
        &volume_uuid,
        &device_identifier,
        &reservation_manifest_json,
        &retained_json,
        &manifest_json,
    )
    .await?;
    let final_manifest = snapshot_inventory_manifest(&device_identifier)?;
    let mut expected_manifest = retained_baseline;
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
async fn persist_prepared_creation(
    journal: &SqlitePool,
    uid: u32,
    lease_id: &str,
    volume: &str,
    device: &str,
    previous_baseline: &str,
    retained_baseline: &str,
    owned_manifest: &str,
) -> Result<(), MountHelperError> {
    let previous = parse_reservation_manifest(previous_baseline)?;
    let mut transaction = journal.begin().await?;
    let updated = sqlx::query("UPDATE mounts SET snapshot_manifest = ?, reservation_manifest = ?, state = 'prepared' WHERE lease_id = ? AND uid = ? AND state = 'reserved' AND snapshot_manifest = '[]' AND reservation_manifest = ? AND reservation_manifest_valid = 1")
        .bind(owned_manifest).bind(retained_baseline).bind(lease_id).bind(i64::from(uid)).bind(previous_baseline)
        .execute(&mut *transaction).await?;
    if updated.rows_affected() != 1 {
        return Err(MountHelperError::Protocol(
            "cleanup reservation was claimed by another request".into(),
        ));
    }
    // Keep terminal audit baselines aligned with this serialized system creation transaction.
    // Only rows already verified by Reserve and still in the exact clean terminal shape qualify.
    let released = sqlx::query("SELECT lease_id, reservation_manifest FROM mounts WHERE uid = ? AND lower(volume_uuid) = lower(?) AND device_identifier = ? AND state = 'released' AND mount_root = '' AND mount_device = 0 AND snapshot_manifest = '[]' AND reservation_manifest_valid = 1")
        .bind(i64::from(uid)).bind(volume).bind(device).fetch_all(&mut *transaction).await?;
    for row in released {
        let old: String = row.get("reservation_manifest");
        if !parse_reservation_manifest(&old)
            .is_ok_and(|baseline| manifests_equal(&baseline, &previous))
        {
            return Err(MountHelperError::Protocol(
                "terminal snapshot baseline changed during creation".into(),
            ));
        }
        sqlx::query("UPDATE mounts SET reservation_manifest = ? WHERE lease_id = ? AND state = 'released' AND reservation_manifest = ?")
            .bind(retained_baseline).bind(row.get::<String,_>("lease_id")).bind(old).execute(&mut *transaction).await?;
    }
    transaction.commit().await?;
    Ok(())
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
        "SELECT COUNT(*) FROM mounts WHERE lower(volume_uuid) = lower(?) AND lease_id != ? AND state IS NOT 'released'",
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
    reconcile_inventory: bool,
    journal: &SqlitePool,
) -> Result<MountReleaseResult, MountHelperError> {
    release_request_with_observation(
        peer_uid,
        uid,
        lease_id,
        reconcile_inventory,
        journal,
        &SystemCleanupObservation,
    )
    .await
}

async fn release_request_with_observation(
    peer_uid: u32,
    uid: u32,
    lease_id: &str,
    reconcile_inventory: bool,
    journal: &SqlitePool,
    observation: &impl CleanupObservation,
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
    if stored_uid != i64::from(uid) {
        return Err(MountHelperError::Protocol(
            "mount lease belongs to another user".into(),
        ));
    }
    let volume_uuid: String = row.try_get("volume_uuid")?;
    let device_identifier: String = row.try_get("device_identifier")?;
    let mount_root: String = row.try_get("mount_root")?;
    let mount_device: i64 = row.try_get("mount_device").unwrap_or_default();
    let manifest_json: String = row.try_get("snapshot_manifest")?;
    let reservation_manifest_json: String = row.try_get("reservation_manifest")?;
    let reservation_manifest_valid: i64 = row.try_get("reservation_manifest_valid")?;
    let state: String = row.try_get("state")?;
    let last_renewed_at: String = row.try_get("last_renewed_at")?;
    // The age gate protects an in-flight `reserved` lease. Once a row has entered
    // `cleanup_pending`, the Access process can no longer renew it, so an explicit
    // retry must be allowed to reconcile it immediately. Keeping the age gate on
    // this terminal state made the retry action a no-op until an unrelated timeout.
    let stale_reserved = state == "reserved"
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
    if !matches!(
        state.as_str(),
        "reserved" | "prepared" | "active" | "cleanup_pending" | "released"
    ) {
        return Err(MountHelperError::Protocol(
            "mount lease has an unknown journal state; explicit repair is required".into(),
        ));
    }
    if state == "released" {
        if !reconcile_inventory {
            return Ok(MountReleaseResult {
                lease_id: lease_id.to_string(),
                cleanup_state: "reconcile_required".to_string(),
            });
        }
        return reconcile_released_tombstone(
            journal,
            uid,
            lease_id,
            &volume_uuid,
            &device_identifier,
            &mount_root,
            &manifest_json,
            &reservation_manifest_json,
            reservation_manifest_valid,
        )
        .await;
    }
    if reservation_manifest_valid != 1 {
        update_state(journal, lease_id, "cleanup_pending").await?;
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: "cleanup_pending".to_string(),
        });
    }
    if (state == "cleanup_pending" || stale_reserved)
        && mount_root.is_empty()
        && manifest_json == "[]"
    {
        if state == "reserved" && !stale_reserved {
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
    if mount_device == 0
        && !mount_root.is_empty()
        && matches!(state.as_str(), "prepared" | "cleanup_pending")
    {
        let cleanup_state = cleanup_mount_with_observation(
            Path::new(&mount_root),
            &manifest,
            None,
            &volume_uuid,
            journal,
            lease_id,
            &device_identifier,
            &reservation_manifest_json,
            &manifest_json,
            observation,
        )
        .await;
        update_state(journal, lease_id, cleanup_state).await?;
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: cleanup_state.to_string(),
        });
    }
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
    if !inventory_matches_expected(&device_identifier, &reservation_manifest_json, &manifest)
        && !inventory_matches_reservation_baseline(&device_identifier, &reservation_manifest_json)
    {
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
        || !validate_cleanup_mount_root(&expected_root, expected_device)
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

/// Re-baseline a released tombstone after an inventory-only change. This is deliberately routed
/// through Release so the root helper revalidates the journal owner, volume/device binding and
/// current inventory while holding the mutation lock. It never deletes a snapshot or opens a
/// source path.
#[allow(clippy::too_many_arguments)]
async fn reconcile_released_tombstone(
    journal: &SqlitePool,
    uid: u32,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    mount_root: &str,
    snapshot_manifest_json: &str,
    reservation_manifest_json: &str,
    reservation_manifest_valid: i64,
) -> Result<MountReleaseResult, MountHelperError> {
    if !mount_root.is_empty() || snapshot_manifest_json != "[]" || reservation_manifest_valid != 1 {
        return Err(MountHelperError::Protocol(
            "released snapshot tombstone is not eligible for inventory reconciliation".into(),
        ));
    }
    validate_uuid(volume_uuid)?;
    validate_device(device_identifier)?;
    validate_device_volume_binding(device_identifier, volume_uuid)?;
    let baseline = parse_reservation_manifest(reservation_manifest_json)?;
    let current_manifest = snapshot_inventory_manifest(device_identifier)?;
    if manifests_equal(&current_manifest, &baseline) {
        return Ok(MountReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: "released".to_string(),
        });
    }
    let current_manifest_json = serde_json::to_string(&current_manifest)?;
    let mut transaction = journal.begin().await?;
    let updated = sqlx::query(
        "UPDATE mounts SET reservation_manifest = ?, reservation_manifest_valid = 1 WHERE lease_id = ? AND uid = ? AND state = 'released' AND mount_root = '' AND snapshot_manifest = '[]' AND reservation_manifest_valid = 1 AND reservation_manifest = ?",
    )
    .bind(&current_manifest_json)
    .bind(lease_id)
    .bind(i64::from(uid))
    .bind(reservation_manifest_json)
    .execute(&mut *transaction)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(MountHelperError::Protocol(
            "released snapshot tombstone changed during reconciliation".into(),
        ));
    }
    let confirmed_manifest = snapshot_inventory_manifest(device_identifier)?;
    if !manifests_equal(&confirmed_manifest, &current_manifest) {
        return Err(MountHelperError::Protocol(
            "released snapshot tombstone inventory changed during reconciliation".into(),
        ));
    }
    transaction.commit().await?;
    Ok(MountReleaseResult {
        lease_id: lease_id.to_string(),
        cleanup_state: "released".to_string(),
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
    ) && !inventory_matches_reservation_baseline(&device_identifier, &reservation_manifest_json)
    {
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
    cleanup_mount_with_observation(
        mount_root,
        manifest,
        expected_device,
        volume_uuid,
        journal,
        lease_id,
        device_identifier,
        reservation_manifest_json,
        manifest_json,
        &SystemCleanupObservation,
    )
    .await
}

trait CleanupObservation {
    fn unmounted_root(&self, root: &Path, name: &str, device: &str, volume: &str) -> bool;
    fn remove_root(&self, root: &Path) -> bool;
    fn inventory(&self, device: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError>;
    fn delete(
        &self,
        volume: &str,
        manifest: &[MountSnapshotRef],
    ) -> Result<(), Vec<MountSnapshotRef>>;
}

struct SystemCleanupObservation;
impl CleanupObservation for SystemCleanupObservation {
    fn unmounted_root(&self, root: &Path, name: &str, device: &str, volume: &str) -> bool {
        validate_legacy_mount_root(root)
            && validate_device_volume_binding(device, volume).is_ok()
            && matches!(mounted_snapshot_path(root), Ok(false))
            && system_mount_table()
                .is_ok_and(|mounts| !mounted_legacy_source_or_root(&mounts, root, name, device))
    }
    fn remove_root(&self, root: &Path) -> bool {
        match fs::remove_dir(root) {
            Ok(()) => true,
            Err(error) => error.kind() == io::ErrorKind::NotFound,
        }
    }
    fn inventory(&self, device: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
        snapshot_inventory_manifest(device)
    }
    fn delete(
        &self,
        volume: &str,
        manifest: &[MountSnapshotRef],
    ) -> Result<(), Vec<MountSnapshotRef>> {
        delete_manifest(volume, manifest)
    }
}

#[allow(clippy::too_many_arguments)]
async fn cleanup_mount_with_observation(
    mount_root: &Path,
    manifest: &[MountSnapshotRef],
    expected_device: Option<u64>,
    volume_uuid: &str,
    journal: &SqlitePool,
    lease_id: &str,
    device_identifier: &str,
    reservation_manifest_json: &str,
    manifest_json: &str,
    observation: &impl CleanupObservation,
) -> &'static str {
    if expected_device.is_none() {
        return cleanup_unmounted_prepared_with_observation(
            mount_root,
            manifest,
            volume_uuid,
            journal,
            lease_id,
            device_identifier,
            reservation_manifest_json,
            manifest_json,
            observation,
        )
        .await;
    }
    let unmount_ok = match mounted_snapshot_path(mount_root) {
        Ok(true)
            if expected_device.is_some_and(|expected| {
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

/// A failed mount has no device identity to unmount. Its recorded snapshots can still be
/// cleaned after proving the exact journal root is unmounted; never infer absence from device=0.
#[allow(clippy::too_many_arguments)]
async fn cleanup_unmounted_prepared_with_observation(
    root: &Path,
    manifest: &[MountSnapshotRef],
    volume: &str,
    journal: &SqlitePool,
    lease: &str,
    device: &str,
    baseline_json: &str,
    manifest_json: &str,
    observation: &impl CleanupObservation,
) -> &'static str {
    let Ok(Some(row)) = sqlx::query(
        "SELECT uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid FROM mounts WHERE lease_id = ? AND mount_device = 0 AND state IN ('prepared', 'cleanup_pending') AND NOT EXISTS (SELECT 1 FROM mounts AS other WHERE other.lease_id != mounts.lease_id AND lower(other.volume_uuid) = lower(mounts.volume_uuid) AND other.state IS NOT 'released')"
    ).bind(lease).fetch_optional(journal).await else { return "cleanup_pending"; };
    let uid: i64 = row.get("uid");
    let name: String = row.get("snapshot_name");
    let valid = uid > 0
        && u32::try_from(uid).is_ok()
        && validate_lease_id(lease).is_ok()
        && validate_uuid(volume).is_ok()
        && validate_device(device).is_ok()
        && validate_snapshot_name(&name).is_ok()
        && expected_mount_root(uid, lease).is_ok_and(|expected| expected == root)
        && row.get::<String, _>("mount_root") == root.to_string_lossy()
        && row
            .get::<String, _>("volume_uuid")
            .eq_ignore_ascii_case(volume)
        && row.get::<String, _>("device_identifier") == device
        && row.get::<String, _>("snapshot_manifest") == manifest_json
        && row.get::<String, _>("reservation_manifest") == baseline_json
        && row.get::<i64, _>("reservation_manifest_valid") == 1
        && parse_reservation_manifest(manifest_json).is_ok_and(|stored| {
            !stored.is_empty()
                && stored.iter().all(|entry| entry.device_identifier == device)
                && manifests_equal(&stored, manifest)
        })
        && parse_reservation_manifest(baseline_json).is_ok();
    if !valid || !observation.unmounted_root(root, &name, device, volume) {
        return "cleanup_pending";
    }
    let Ok(mut expected_inventory) = parse_reservation_manifest(baseline_json) else {
        return "cleanup_pending";
    };
    let baseline = expected_inventory.clone();
    expected_inventory.extend_from_slice(manifest);
    if !observation.inventory(device).is_ok_and(|current| {
        manifests_equal(&current, &expected_inventory) || manifests_equal(&current, &baseline)
    }) {
        return "cleanup_pending";
    }
    // Only an empty, root-owned managed directory is removed. Repeat kernel/volume checks
    // immediately before snapshot deletion; a substituted or mounted root stays blocked.
    if !observation.remove_root(root) || !observation.unmounted_root(root, &name, device, volume) {
        return "cleanup_pending";
    }
    delete_manifest_and_record_tombstone_with_observation(
        journal,
        lease,
        volume,
        device,
        baseline_json,
        manifest_json,
        manifest,
        observation,
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

/// Validate a legacy mount root without requiring its historical parent directories to still
/// exist. Older helpers could leave journal rows after the per-UID directory had been removed;
/// absence below the root-owned base is itself safe evidence that there is no mount to unmount.
/// Existing components are still checked for root ownership, non-writability and non-symlink
/// identity before any cleanup is attempted.
fn validate_managed_mount_ancestors(path: &Path) -> bool {
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
    let uid_root_ok = match fs::symlink_metadata(uid_root) {
        Ok(metadata) => {
            metadata.is_dir()
                && metadata.uid() == 0
                && metadata.mode() & 0o022 == 0
                && !metadata.file_type().is_symlink()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    base_ok && uid_root_ok && path.starts_with(&base)
}

fn validate_legacy_mount_root(path: &Path) -> bool {
    let mount_root_ok = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            metadata.is_dir()
                && metadata.uid() == 0
                && metadata.mode() & 0o022 == 0
                && !metadata.file_type().is_symlink()
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    };
    validate_managed_mount_ancestors(path) && mount_root_ok
}

// Mounted root metadata belongs to the source filesystem (Data commonly has mode 0775),
// not to the protected empty mountpoint. Keep ancestor security and pin the mounted device.
fn cleanup_root_metadata_valid(
    metadata: &fs::Metadata,
    parent_device: u64,
    expected_device: Option<u64>,
) -> bool {
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return false;
    }
    if metadata.dev() != parent_device {
        return expected_device == Some(metadata.dev());
    }
    metadata.uid() == 0 && metadata.mode() & 0o022 == 0
}

fn validate_cleanup_mount_root(path: &Path, expected_device: Option<u64>) -> bool {
    if !validate_managed_mount_ancestors(path) {
        return false;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => path
            .parent()
            .and_then(|parent| fs::symlink_metadata(parent).ok())
            .is_some_and(|parent| {
                cleanup_root_metadata_valid(&metadata, parent.dev(), expected_device)
            }),
        Err(error) => error.kind() == io::ErrorKind::NotFound,
    }
}

#[cfg(any(target_os = "macos", test))]
fn legacy_mount_identity_matches(
    mounted_from: &str,
    snapshot_name: &str,
    device_identifier: &str,
) -> bool {
    let expected = format!("{snapshot_name}@/dev/{device_identifier}");
    mounted_from == expected
}

/// Return the mounted filesystem's device number only when the managed path is provably the
/// journaled APFS snapshot. The old journal did not persist `mount_device`, so checking only
/// `stat(2)` would allow a different privileged mount at the same path to be unmounted. macOS
/// exposes the APFS snapshot source through `statfs(2)` as `<snapshot-name>@/dev/<volume>`; the
/// exact source and mount point are compared before any Release call is allowed.
fn legacy_mounted_snapshot_device(
    path: &Path,
    snapshot_name: &str,
    device_identifier: &str,
) -> Result<Option<u64>, MountHelperError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(MountHelperError::Io(error)),
    };
    let parent = path
        .parent()
        .ok_or_else(|| MountHelperError::Protocol("mount root has no parent".into()))?;
    let parent_metadata = match fs::metadata(parent) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(MountHelperError::Io(error)),
    };
    if metadata.dev() == parent_metadata.dev() {
        return Ok(None);
    }
    #[cfg(target_os = "macos")]
    {
        let mut filesystem = std::mem::MaybeUninit::<libc::statfs>::uninit();
        let path_bytes = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| MountHelperError::Protocol("invalid managed mount root".into()))?;
        if unsafe { libc::statfs(path_bytes.as_ptr(), filesystem.as_mut_ptr()) } != 0 {
            return Err(MountHelperError::Io(std::io::Error::last_os_error()));
        }
        let filesystem = unsafe { filesystem.assume_init() };
        let from = filesystem
            .f_mntfromname
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        let mount_point = filesystem
            .f_mntonname
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        let filesystem_type = filesystem
            .f_fstypename
            .iter()
            .take_while(|byte| **byte != 0)
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        let from = std::str::from_utf8(&from)
            .map_err(|_| MountHelperError::Protocol("invalid mounted snapshot source".into()))?;
        let mount_point = std::str::from_utf8(&mount_point)
            .map_err(|_| MountHelperError::Protocol("invalid mounted snapshot path".into()))?;
        let filesystem_type = std::str::from_utf8(&filesystem_type)
            .map_err(|_| MountHelperError::Protocol("invalid mounted filesystem type".into()))?;
        let canonical_path = fs::canonicalize(path)?;
        let canonical_mount_point = fs::canonicalize(mount_point)?;
        if filesystem_type.eq_ignore_ascii_case("apfs")
            && legacy_mount_identity_matches(from, snapshot_name, device_identifier)
            && canonical_mount_point == canonical_path
        {
            return Ok(Some(metadata.dev()));
        }
        Err(MountHelperError::Protocol(
            "managed mount root is not the recorded APFS snapshot".into(),
        ))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (snapshot_name, device_identifier);
        Err(MountHelperError::Protocol(
            "legacy mounted snapshot identity requires macOS".into(),
        ))
    }
}

/// Clean a legacy row after proving that its recorded root is not mounted. This is separate from
/// `cleanup_mount`: legacy repair has already migrated its identity, whereas normal failed-mount
/// cleanup additionally binds an unmounted root to the exact prepared journal row and kernel
/// mount table. Neither path unmounts a filesystem without a proven mounted identity.
#[allow(clippy::too_many_arguments)]
async fn cleanup_legacy_unmounted_root(
    mount_root: &Path,
    manifest: &[MountSnapshotRef],
    volume_uuid: &str,
    journal: &SqlitePool,
    lease_id: &str,
    device_identifier: &str,
    reservation_manifest_json: &str,
    manifest_json: &str,
) -> &'static str {
    match mounted_snapshot_path(mount_root) {
        Ok(false) => {}
        Ok(true) | Err(_) => return "cleanup_pending",
    }
    match fs::remove_dir(mount_root) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return "cleanup_pending",
    }
    if mount_root.exists() {
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

async fn delete_manifest_and_record_tombstone(
    journal: &SqlitePool,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    reservation_manifest_json: &str,
    manifest_json: &str,
    manifest: &[MountSnapshotRef],
) -> &'static str {
    delete_manifest_and_record_tombstone_with_observation(
        journal,
        lease_id,
        volume_uuid,
        device_identifier,
        reservation_manifest_json,
        manifest_json,
        manifest,
        &SystemCleanupObservation,
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn delete_manifest_and_record_tombstone_with_observation(
    journal: &SqlitePool,
    lease_id: &str,
    volume_uuid: &str,
    device_identifier: &str,
    reservation_manifest_json: &str,
    manifest_json: &str,
    manifest: &[MountSnapshotRef],
    observation: &impl CleanupObservation,
) -> &'static str {
    let reservation_manifest = match parse_reservation_manifest(reservation_manifest_json) {
        Ok(manifest) => manifest,
        Err(_) => return "cleanup_pending",
    };
    // A previous process may have deleted the recorded snapshots and crashed before it could
    // persist the released tombstone. An exact reservation-baseline inventory proves that the
    // recorded snapshots are already absent, so the journal transition can be retried safely.
    if let Ok(current_manifest) = observation.inventory(device_identifier)
        && manifests_equal(&current_manifest, &reservation_manifest)
    {
        return record_released_tombstone(
            journal,
            lease_id,
            reservation_manifest_json,
            manifest_json,
        )
        .await;
    }
    let mut expected = reservation_manifest.clone();
    expected.extend_from_slice(manifest);
    if !observation
        .inventory(device_identifier)
        .is_ok_and(|current| manifests_equal(&current, &expected))
    {
        return "cleanup_pending";
    }
    let confirmed_deleted = observation.delete(volume_uuid, manifest).err();
    if let Some(confirmed_deleted) = confirmed_deleted {
        retain_unconfirmed_manifest_after_delete_failure(
            observation,
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
    let current_manifest = match observation.inventory(device_identifier) {
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
    record_released_tombstone(journal, lease_id, reservation_manifest_json, manifest_json).await
}

async fn record_released_tombstone(
    journal: &SqlitePool,
    lease_id: &str,
    reservation_manifest_json: &str,
    manifest_json: &str,
) -> &'static str {
    let updated = sqlx::query(
        "UPDATE mounts SET mount_root = '', mount_device = 0, snapshot_manifest = '[]', reservation_manifest = ?, reservation_manifest_valid = 1, state = 'released' WHERE lease_id = ? AND snapshot_manifest = ? AND reservation_manifest_valid = 1",
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

#[allow(clippy::too_many_arguments)]
async fn retain_unconfirmed_manifest_after_delete_failure(
    observation: &impl CleanupObservation,
    journal: &SqlitePool,
    lease_id: &str,
    device_identifier: &str,
    manifest_json: &str,
    manifest: &[MountSnapshotRef],
    confirmed_deleted: &[MountSnapshotRef],
) {
    let remaining_manifest = match observation.inventory(device_identifier) {
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

/// Split the current inventory into the old journal's recorded snapshots and the snapshots that
/// pre-dated that journal entry. Legacy rows did not persist the reservation baseline, so this is
/// the only safe migration input: every current snapshot is either an exact recorded entry or is
/// retained as the new baseline. Missing recorded entries are treated as already deleted and are
/// never recreated or guessed at.
#[cfg(test)]
fn infer_legacy_reservation_baseline(
    current_manifest: &[MountSnapshotRef],
    recorded_manifest: &[MountSnapshotRef],
) -> (Vec<MountSnapshotRef>, Vec<MountSnapshotRef>) {
    let mut baseline = current_manifest.to_vec();
    let mut present_recorded = Vec::new();
    for recorded in recorded_manifest {
        if let Some(index) = baseline.iter().position(|current| {
            current
                .device_identifier
                .eq_ignore_ascii_case(&recorded.device_identifier)
                && current.uuid.eq_ignore_ascii_case(&recorded.uuid)
        }) {
            present_recorded.push(baseline.remove(index));
        }
    }
    (baseline, present_recorded)
}

/// Reconcile a legacy row against the current volume inventory using snapshot UUIDs. The old
/// journal stored device nodes in each entry, so comparing the whole pair would make every row
/// look foreign after a device-node renumbering. A duplicate UUID is ambiguous and is rejected;
/// snapshots not present in the current inventory are already gone and are not sent to delete.
fn partition_legacy_manifests(
    current_manifest: &[MountSnapshotRef],
    stored_baseline: Option<&[MountSnapshotRef]>,
    recorded_manifest: &[MountSnapshotRef],
) -> Option<(Vec<MountSnapshotRef>, Vec<MountSnapshotRef>)> {
    let mut inventory_uuids = std::collections::HashSet::new();
    if current_manifest
        .iter()
        .any(|snapshot| !inventory_uuids.insert(snapshot.uuid.to_ascii_lowercase()))
    {
        return None;
    }
    let mut current = current_manifest.to_vec();
    let mut baseline = Vec::new();
    let mut release = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for snapshot in stored_baseline.into_iter().flatten() {
        let key = snapshot.uuid.to_ascii_lowercase();
        if !seen.insert(key.clone()) {
            return None;
        }
        if let Some(index) = current
            .iter()
            .position(|candidate| candidate.uuid.eq_ignore_ascii_case(&snapshot.uuid))
        {
            baseline.push(current.remove(index));
        }
    }
    for snapshot in recorded_manifest {
        let key = snapshot.uuid.to_ascii_lowercase();
        if !seen.insert(key) {
            return None;
        }
        if let Some(index) = current
            .iter()
            .position(|candidate| candidate.uuid.eq_ignore_ascii_case(&snapshot.uuid))
        {
            release.push(current.remove(index));
        }
    }
    baseline.extend(current);
    Some((baseline, release))
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

fn legacy_reservation_baseline(
    value: &str,
    valid: i64,
) -> Result<Option<Vec<MountSnapshotRef>>, MountHelperError> {
    match valid {
        0 => Ok(None),
        1 => parse_reservation_manifest(value).map(Some),
        _ => Err(MountHelperError::Protocol(
            "snapshot mount journal contains an invalid reservation marker".into(),
        )),
    }
}

fn parse_reservation_manifest(value: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
    let manifest = serde_json::from_str::<Vec<MountSnapshotRef>>(value).map_err(|_| {
        MountHelperError::Protocol("snapshot mount journal contains an invalid reservation".into())
    })?;
    let mut seen = std::collections::HashSet::new();
    for snapshot in &manifest {
        validate_device(&snapshot.device_identifier)?;
        validate_uuid(&snapshot.uuid)?;
        if !seen.insert(snapshot.uuid.to_ascii_lowercase()) {
            return Err(MountHelperError::Protocol(
                "snapshot mount journal contains duplicate reservation UUIDs".into(),
            ));
        }
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

fn inventory_matches_reservation_baseline(
    device_identifier: &str,
    reservation_manifest_json: &str,
) -> bool {
    let Ok(reservation_manifest) = parse_reservation_manifest(reservation_manifest_json) else {
        return false;
    };
    snapshot_inventory_manifest(device_identifier)
        .is_ok_and(|current_manifest| manifests_equal(&current_manifest, &reservation_manifest))
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

/// Resolve a legacy journal row to the currently mounted APFS device. Device identifiers such as
/// `disk3s1` are allocation names, not durable identities; the volume UUID is the durable binding
/// recorded by Snapshot Access. The historical device is accepted only when it still validates;
/// otherwise `diskutil info` is queried by UUID and the returned APFS identity is checked before
/// it is used for inventory or Release.
fn resolve_legacy_volume_device(
    volume_uuid: &str,
    historical_device: &str,
) -> Result<String, MountHelperError> {
    validate_uuid(volume_uuid)?;
    if validate_device(historical_device).is_ok()
        && validate_device_volume_binding(historical_device, volume_uuid).is_ok()
    {
        return Ok(historical_device.to_string());
    }
    let output = run_command_output(
        "volume_identity",
        "diskutil.info",
        "/usr/sbin/diskutil",
        ["info", "-plist", volume_uuid],
    )?;
    let (device_identifier, actual_volume_uuid) = parse_volume_identity(&output)?;
    if !actual_volume_uuid.eq_ignore_ascii_case(volume_uuid) {
        return Err(MountHelperError::Protocol(
            "resolved APFS volume UUID does not match the journal".into(),
        ));
    }
    validate_device(&device_identifier)?;
    Ok(device_identifier)
}

fn parse_volume_identity(output: &[u8]) -> Result<(String, String), MountHelperError> {
    let value = Value::from_reader_xml(output).map_err(|_| {
        MountHelperError::Protocol(
            "stage=volume_identity command=diskutil.info exit=0 channel=stdout summary=invalid volume plist".into(),
        )
    })?;
    let dict = value.as_dictionary().ok_or_else(|| {
        MountHelperError::Protocol(
            "stage=volume_identity command=diskutil.info exit=0 channel=stdout summary=volume plist is not a dictionary".into(),
        )
    })?;
    let filesystem = dict
        .get("FilesystemName")
        .and_then(Value::as_string)
        .unwrap_or_default();
    let device_identifier = dict
        .get("DeviceIdentifier")
        .and_then(Value::as_string)
        .unwrap_or_default();
    let actual_volume_uuid = dict
        .get("APFSVolumeUUID")
        .and_then(Value::as_string)
        .or_else(|| dict.get("VolumeUUID").and_then(Value::as_string))
        .unwrap_or_default();
    if !filesystem.eq_ignore_ascii_case("APFS")
        || validate_device(device_identifier).is_err()
        || validate_uuid(actual_volume_uuid).is_err()
    {
        return Err(MountHelperError::Protocol(
            "resolved volume is not a valid APFS volume identity".into(),
        ));
    }
    Ok((
        device_identifier.to_string(),
        actual_volume_uuid.to_string(),
    ))
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
    let (actual_device, actual_volume_uuid) = parse_volume_identity(&output).map_err(|_| {
        MountHelperError::Protocol(format!(
            "stage={stage} command=diskutil.info exit=0 channel=stdout summary=invalid volume plist"
        ))
    })?;
    if !actual_device.eq_ignore_ascii_case(device_identifier)
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
    let mut seen = std::collections::HashSet::new();
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
        if !seen.insert(uuid.to_ascii_lowercase()) {
            return Err(MountHelperError::Protocol(
                "stage=snapshot_inventory command=diskutil.apfs.listSnapshots exit=0 channel=stdout summary=duplicate snapshot UUID".into(),
            ));
        }
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
    let timeout = matches!(stage, "snapshot_inventory" | "volume_identity")
        .then_some(STATUS_COMMAND_TIMEOUT)
        .unwrap_or(MUTATING_REQUEST_TIMEOUT);
    let mut command_builder = Command::new(program);
    command_builder
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    // Keep descendants in a private process group so a timeout cannot leave a child holding
    // either output pipe open after the direct command is killed.
    unsafe {
        command_builder.pre_exec(|| {
            if libc::setpgid(0, 0) == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    let mut child = command_builder
        .spawn()
        .map_err(|error| MountHelperError::Command {
            stage,
            command,
            status: "not_started".into(),
            channel: "errno",
            summary: format!("errno={}", error.raw_os_error().unwrap_or(0)),
        })?;
    let mut stdout = child.stdout.take().ok_or_else(|| {
        terminate_command_group(&mut child);
        MountHelperError::Protocol("snapshot command stdout was unavailable".into())
    })?;
    let mut stderr = child.stderr.take().ok_or_else(|| {
        terminate_command_group(&mut child);
        MountHelperError::Protocol("snapshot command stderr was unavailable".into())
    })?;
    set_nonblocking(stdout.as_raw_fd()).map_err(|error| {
        terminate_command_group(&mut child);
        MountHelperError::Io(error)
    })?;
    set_nonblocking(stderr.as_raw_fd()).map_err(|error| {
        terminate_command_group(&mut child);
        MountHelperError::Io(error)
    })?;
    let mut stdout_bytes = Vec::new();
    let mut stderr_bytes = Vec::new();
    let mut stdout_closed = false;
    let mut stderr_closed = false;
    let mut status = None;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if !stdout_closed {
            stdout_closed = match drain_command_pipe(&mut stdout, &mut stdout_bytes, deadline) {
                Ok(closed) => closed,
                Err(error) => {
                    terminate_command_group(&mut child);
                    return Err(MountHelperError::Io(error));
                }
            };
        }
        if !stderr_closed {
            stderr_closed = match drain_command_pipe(&mut stderr, &mut stderr_bytes, deadline) {
                Ok(closed) => closed,
                Err(error) => {
                    terminate_command_group(&mut child);
                    return Err(MountHelperError::Io(error));
                }
            };
        }
        if status.is_none() {
            status = match child.try_wait() {
                Ok(status) => status,
                Err(error) => {
                    terminate_command_group(&mut child);
                    return Err(MountHelperError::Io(error));
                }
            };
        }
        if status.is_some() && stdout_closed && stderr_closed {
            break;
        }
        if std::time::Instant::now() >= deadline {
            terminate_command_group(&mut child);
            return Err(MountHelperError::Command {
                stage,
                command,
                status: "timeout".into(),
                channel: "timeout",
                summary: format!("timeout={}ms", timeout.as_millis()),
            });
        }
        thread::sleep(COMMAND_POLL_INTERVAL);
    }
    let status = status.expect("command status is set before output pipes close");
    let output = std::process::Output {
        status,
        stdout: stdout_bytes,
        stderr: stderr_bytes,
    };
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(command_failure(stage, command, &output))
}

fn set_nonblocking(fd: RawFd) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn drain_command_pipe<R: Read>(
    reader: &mut R,
    output: &mut Vec<u8>,
    deadline: std::time::Instant,
) -> io::Result<bool> {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        if std::time::Instant::now() >= deadline {
            return Ok(false);
        }
        match reader.read(&mut buffer) {
            Ok(0) => return Ok(true),
            Ok(count) => {
                let remaining = MAX_COMMAND_OUTPUT_BYTES.saturating_sub(output.len());
                if remaining > 0 {
                    output.extend_from_slice(&buffer[..count.min(remaining)]);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => return Ok(false),
            Err(error) => return Err(error),
        }
    }
}

fn terminate_command_group(child: &mut Child) {
    let pid = child.id() as libc::pid_t;
    if pid > 0 {
        // The child was placed in a private process group in pre_exec. Kill the group first so
        // descendants cannot keep the nonblocking pipes or root-helper mutation lock alive.
        unsafe {
            let _ = libc::kill(-pid, libc::SIGKILL);
        }
    }
    let _ = child.kill();
    let _ = child.wait();
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
    fn snapshot_creation_receipt_binds_the_owned_snapshot_name() {
        assert_eq!(
            created_snapshot_name(b"Created local snapshot with date: 2026-01-02-030405\n")
                .unwrap(),
            "com.apple.TimeMachine.2026-01-02-030405.local"
        );
        for invalid in [b"".as_slice(), b"Created local snapshot with date: ../../other", b"Created local snapshot with date: 2026-01-02-030405\nCreated local snapshot with date: 2026-01-02-030406\n"] {
            assert!(created_snapshot_name(invalid).is_err());
        }
    }

    #[tokio::test]
    async fn creation_pruning_rebases_only_clean_terminal_rows_and_keeps_owned_uuid() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let volume = Uuid::new_v4().to_string();
        let retained = MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: Uuid::new_v4().to_string(),
        };
        let pruned = MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: Uuid::new_v4().to_string(),
        };
        let owned = MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: Uuid::new_v4().to_string(),
        };
        let before = serde_json::to_string(&vec![retained.clone(), pruned]).unwrap();
        let after = serde_json::to_string(&vec![retained.clone()]).unwrap();
        let manifest = serde_json::to_string(&vec![owned.clone()]).unwrap();
        let lease = Uuid::new_v4().to_string();
        for (id, state, uid) in [
            (lease.clone(), "reserved", 501),
            (Uuid::new_v4().to_string(), "released", 501),
            (Uuid::new_v4().to_string(), "released", 502),
        ] {
            sqlx::query("INSERT INTO mounts (lease_id,uid,volume_uuid,device_identifier,snapshot_name,mount_root,snapshot_manifest,reservation_manifest,reservation_manifest_valid,state,created_at,last_renewed_at) VALUES (?,?,?,'disk5s1','','','[]',?,1,?,datetime('now'),datetime('now'))")
                .bind(id).bind(uid).bind(&volume).bind(&before).bind(state).execute(&journal).await.unwrap();
        }
        persist_prepared_creation(
            &journal, 501, &lease, &volume, "disk5s1", &before, &after, &manifest,
        )
        .await
        .unwrap();
        let rows =
            sqlx::query("SELECT uid,state,reservation_manifest,snapshot_manifest FROM mounts")
                .fetch_all(&journal)
                .await
                .unwrap();
        for row in rows {
            let baseline: String = row.get("reservation_manifest");
            if row.get::<i64, _>("uid") == 502 {
                assert_eq!(baseline, before);
            } else {
                assert_eq!(baseline, after);
            }
            if row.get::<String, _>("state") == "prepared" {
                assert_eq!(row.get::<String, _>("snapshot_manifest"), manifest);
            }
        }
        assert!(
            persist_prepared_creation(
                &journal, 501, &lease, &volume, "disk5s1", &before, &after, &manifest
            )
            .await
            .is_err()
        );
        let current = vec![retained, owned.clone()];
        let previous = parse_reservation_manifest(&before).unwrap();
        let (baseline, release) =
            partition_legacy_manifests(&current, Some(&previous), &[owned]).unwrap();
        assert_eq!(baseline.len(), 1);
        assert_eq!(release.len(), 1);
    }

    #[test]
    fn mounted_data_root_permissions_do_not_prevent_exact_device_cleanup() {
        let temp = tempfile::tempdir().unwrap();
        fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o775)).unwrap();
        let metadata = fs::symlink_metadata(temp.path()).unwrap();
        let device = metadata.dev();
        assert!(cleanup_root_metadata_valid(
            &metadata,
            device.wrapping_add(1),
            Some(device)
        ));
        assert!(!cleanup_root_metadata_valid(
            &metadata,
            device.wrapping_add(1),
            Some(device.wrapping_add(2))
        ));
        assert!(!cleanup_root_metadata_valid(
            &metadata,
            device.wrapping_add(1),
            None
        ));
        assert!(!cleanup_root_metadata_valid(
            &metadata,
            device,
            Some(device)
        ));
    }

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
                reconcile_inventory: false,
            }),
            MUTATING_REQUEST_TIMEOUT
        );
        assert_eq!(
            request_timeout(&MountMethod::RepairLegacy { uid: 501 }),
            MUTATING_REQUEST_TIMEOUT
        );
    }

    #[tokio::test]
    async fn released_tombstone_clears_verified_cleanup_mount_identity() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let lease_id = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, mount_device, snapshot_manifest, reservation_manifest_valid, state, created_at) VALUES (?, 501, '', 'disk5s1', '', '/previously-verified-root', 42, '[]', 1, 'cleanup_pending', datetime('now'))")
            .bind(&lease_id).execute(&journal).await.unwrap();
        assert_eq!(
            record_released_tombstone(&journal, &lease_id, "[]", "[]").await,
            "released"
        );
        let row = sqlx::query("SELECT mount_root, mount_device FROM mounts WHERE lease_id = ?")
            .bind(&lease_id)
            .fetch_one(&journal)
            .await
            .unwrap();
        assert_eq!(row.get::<String, _>("mount_root"), "");
        assert_eq!(row.get::<i64, _>("mount_device"), 0);
    }

    #[tokio::test]
    #[ignore = "explicit administrator-authorized read-only installed-journal diagnostic"]
    async fn readonly_installed_historical_retirement_audit() {
        let uid: u32 = std::env::var("TELEVYBACKUP_AUDIT_UID")
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(uid, 0);
        let expected: usize = std::env::var("TELEVYBACKUP_AUDIT_EXPECTED")
            .unwrap()
            .parse()
            .unwrap();
        let journal = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(DEFAULT_MOUNT_HELPER_JOURNAL)
                    .read_only(true),
            )
            .await
            .unwrap();
        sqlx::query("PRAGMA query_only = ON")
            .execute(&journal)
            .await
            .unwrap();
        let rows = sqlx::query("SELECT * FROM mounts WHERE uid = ? AND state = 'released' AND mount_root != '' AND mount_device = 0 AND last_renewed_at <= datetime('now', '-300 seconds')")
            .bind(i64::from(uid)).fetch_all(&journal).await.unwrap();
        let mut eligible = 0;
        let mut metadata_rejected = 0;
        let mut verification_rejected = 0;
        for row in &rows {
            let lease: String = row.get("lease_id");
            let root: String = row.get("mount_root");
            if !historical_root_metadata_safe(Path::new(&root), uid, &lease) {
                metadata_rejected += 1;
                continue;
            }
            let volume: String = row.get("volume_uuid");
            let historical_device: String = row.get("device_identifier");
            let observation = SystemLegacyRepairObservation;
            let device = observation
                .resolve_device(&volume, &historical_device)
                .unwrap();
            let manifest: String = row.get("snapshot_manifest");
            let name: String = row.get("snapshot_name");
            let baseline: String = row.get("reservation_manifest");
            let marker: i64 = row.get("reservation_manifest_valid");
            let manifest = parse_journal_manifest(&manifest).unwrap();
            match verified_historical_retirement_baseline(
                uid,
                &lease,
                &volume,
                &historical_device,
                &device,
                &name,
                &root,
                &baseline,
                marker,
                &manifest,
                &observation,
            ) {
                Ok(Some(_)) => eligible += 1,
                _ => verification_rejected += 1,
            }
        }
        println!(
            "readonly audit: rows={} eligible={} metadata_rejected={} verification_rejected={}",
            rows.len(),
            eligible,
            metadata_rejected,
            verification_rejected
        );
        assert_eq!(rows.len(), expected);
        assert_eq!(
            eligible, expected,
            "all current historical blockers must pass the production read-only predicates"
        );
    }

    #[derive(Default)]
    struct FailedMountFixture {
        current: std::sync::Mutex<Vec<MountSnapshotRef>>,
        root_safe: bool,
        fail_recheck: bool,
        fail_delete: bool,
        drift_on_inventory_call: Option<usize>,
        proofs: std::sync::atomic::AtomicUsize,
        inventory_calls: std::sync::atomic::AtomicUsize,
        removals: std::sync::atomic::AtomicUsize,
        deletes: std::sync::atomic::AtomicUsize,
    }
    impl CleanupObservation for FailedMountFixture {
        fn unmounted_root(&self, _: &Path, _: &str, _: &str, _: &str) -> bool {
            let call = self
                .proofs
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.root_safe && !(self.fail_recheck && call > 0)
        }
        fn remove_root(&self, _: &Path) -> bool {
            self.removals
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            true
        }
        fn inventory(&self, _: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
            let call = self
                .inventory_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut inventory = self.current.lock().unwrap().clone();
            if self.drift_on_inventory_call.is_some_and(|at| call >= at) {
                inventory.push(MountSnapshotRef {
                    device_identifier: "disk5s1".into(),
                    uuid: "E8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
                });
            }
            Ok(inventory)
        }
        fn delete(
            &self,
            _: &str,
            manifest: &[MountSnapshotRef],
        ) -> Result<(), Vec<MountSnapshotRef>> {
            self.deletes
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.fail_delete {
                return Err(Vec::new());
            }
            self.current
                .lock()
                .unwrap()
                .retain(|entry| !manifest.iter().any(|owned| owned.uuid == entry.uuid));
            Ok(())
        }
    }
    impl LegacyRepairObservation for FailedMountFixture {
        fn binding_valid(&self, _: &str, _: &str) -> bool {
            true
        }
        fn resolve_device(&self, _: &str, _: &str) -> Result<String, MountHelperError> {
            Ok("disk5s1".into())
        }
        fn inventory(&self, device: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
            CleanupObservation::inventory(self, device)
        }
        fn historical_root_unmounted(
            &self,
            _: &Path,
            _: u32,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<bool, MountHelperError> {
            Ok(false)
        }
    }

    // Models the production failure: RecordCleanup has persisted one owned snapshot,
    // mount_apfs is denied, and no mount-device identity was ever obtained.
    async fn denied_mount_cleanup_scenario(retry: bool) {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let lease = Uuid::new_v4().to_string();
        let root = expected_mount_root(501, &lease).unwrap();
        let volume = "B8A38394-49AE-42F9-85FB-14DD9EA549FA";
        let baseline = vec![MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: Uuid::new_v4().to_string(),
        }];
        let owned = vec![MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: Uuid::new_v4().to_string(),
        }];
        let baseline_json = serde_json::to_string(&baseline).unwrap();
        let manifest_json = serde_json::to_string(&owned).unwrap();
        for _ in 0..65 {
            sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?,501,?,'disk5s1','','','[]',?,1,'released',datetime('now','-1 day'),datetime('now','-1 day'))")
                .bind(Uuid::new_v4().to_string()).bind(volume).bind(&baseline_json).execute(&journal).await.unwrap();
        }
        sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, mount_device, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?,501,?,'disk5s1','com.apple.TimeMachine.synthetic',?,0,?,?,1,'cleanup_pending',datetime('now'),datetime('now'))")
            .bind(&lease).bind(volume).bind(root.to_str().unwrap()).bind(&manifest_json).bind(&baseline_json).execute(&journal).await.unwrap();
        let mut inventory = baseline.clone();
        inventory.extend(owned.clone());
        let fixture = FailedMountFixture {
            current: std::sync::Mutex::new(inventory),
            root_safe: true,
            ..Default::default()
        };
        let before = classify_status_rows_with_observation(&journal, Some(501), None, &fixture)
            .await
            .unwrap();
        assert!(!manifests_equal(
            &fixture.current.lock().unwrap(),
            &baseline
        ));
        assert_eq!(
            before.inventory_drift_mounts, 0,
            "the extra snapshot has one exact journal owner"
        );
        assert_eq!(
            before.active_mounts, 1,
            "CLI pending-only guard must accept the retry"
        );
        assert_eq!(before.pending_cleanup, 1);
        assert_eq!(before.pending_cleanup_leases.len(), 1);
        if retry {
            assert!(
                release_request_with_observation(502, 501, &lease, false, &journal, &fixture)
                    .await
                    .is_err()
            );
            let result =
                release_request_with_observation(501, 501, &lease, false, &journal, &fixture)
                    .await
                    .unwrap();
            assert_eq!(result.cleanup_state, "released");
        } else {
            let cleanup = cleanup_mount_with_observation(
                &root,
                &owned,
                None,
                volume,
                &journal,
                &lease,
                "disk5s1",
                &baseline_json,
                &manifest_json,
                &fixture,
            )
            .await;
            assert_eq!(
                cleanup, "released",
                "mount denied before device capture must not make cleanup permanently impossible"
            );
            update_state(&journal, &lease, cleanup).await.unwrap();
        }
        let after = classify_status_rows_with_observation(&journal, Some(501), None, &fixture)
            .await
            .unwrap();
        assert_eq!(
            after.active_mounts, 0,
            "next acquire's source-volume status must no longer block"
        );
        assert!(after.blocked_volume_uuids.is_empty());
        assert!(manifests_equal(&fixture.current.lock().unwrap(), &baseline));
        assert_eq!(fixture.deletes.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    // Models the production failure: RecordCleanup has persisted one owned snapshot,
    // mount_apfs is denied, and no mount-device identity was ever obtained.
    #[tokio::test]
    async fn denied_mount_cleanup_restores_65_tombstones_and_unblocks_source_volume() {
        denied_mount_cleanup_scenario(false).await;
    }

    #[tokio::test]
    async fn denied_mount_release_retry_restores_65_tombstones_and_unblocks_source_volume() {
        denied_mount_cleanup_scenario(true).await;
    }

    #[tokio::test]
    async fn denied_mount_cleanup_keeps_unsafe_or_racing_rows_blocked() {
        for case in [
            "mounted_or_changed_device",
            "mount_race",
            "inventory_race",
            "delete_denied",
            "foreign_uid",
            "wrong_root",
            "corrupt_manifest",
            "duplicate_uuid",
            "invalid_baseline",
            "other_volume_owner",
            "active_without_identity",
        ] {
            let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
            init_journal(&journal).await.unwrap();
            let lease = Uuid::new_v4().to_string();
            let root = expected_mount_root(501, &lease).unwrap();
            let volume = "B8A38394-49AE-42F9-85FB-14DD9EA549FA";
            let owned = vec![MountSnapshotRef {
                device_identifier: "disk5s1".into(),
                uuid: Uuid::new_v4().to_string(),
            }];
            let manifest = serde_json::to_string(&owned).unwrap();
            sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, mount_device, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?,501,?,'disk5s1','com.apple.TimeMachine.synthetic',?,0,?,'[]',1,'cleanup_pending',datetime('now'),datetime('now'))")
                .bind(&lease).bind(volume).bind(root.to_str().unwrap()).bind(&manifest).execute(&journal).await.unwrap();
            let mut fixture = FailedMountFixture {
                current: std::sync::Mutex::new(owned.clone()),
                root_safe: true,
                ..Default::default()
            };
            match case {
                "mounted_or_changed_device" => fixture.root_safe = false,
                "mount_race" => fixture.fail_recheck = true,
                "inventory_race" => fixture.drift_on_inventory_call = Some(1),
                "delete_denied" => fixture.fail_delete = true,
                "foreign_uid" => {
                    sqlx::query("UPDATE mounts SET uid = 502")
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "wrong_root" => {
                    sqlx::query("UPDATE mounts SET mount_root = '/unmanaged'")
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "corrupt_manifest" => {
                    sqlx::query("UPDATE mounts SET snapshot_manifest = 'broken'")
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "duplicate_uuid" => {
                    let duplicate =
                        serde_json::to_string(&[owned[0].clone(), owned[0].clone()]).unwrap();
                    sqlx::query("UPDATE mounts SET snapshot_manifest = ?")
                        .bind(duplicate)
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "invalid_baseline" => {
                    sqlx::query("UPDATE mounts SET reservation_manifest_valid = 0")
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "other_volume_owner" => {
                    sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at, last_renewed_at) VALUES (?,502,?,'disk5s1','','','[]','reserved',datetime('now'),datetime('now'))").bind(Uuid::new_v4().to_string()).bind(volume).execute(&journal).await.unwrap();
                }
                "active_without_identity" => {
                    sqlx::query("UPDATE mounts SET state = 'active'")
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let result = cleanup_mount_with_observation(
                &root, &owned, None, volume, &journal, &lease, "disk5s1", "[]", &manifest, &fixture,
            )
            .await;
            assert_eq!(result, "cleanup_pending", "{case}");
            assert_eq!(
                fixture.current.lock().unwrap().len(),
                1,
                "{case}: preserve owned snapshot"
            );
            if case != "delete_denied" {
                assert_eq!(
                    fixture.deletes.load(std::sync::atomic::Ordering::SeqCst),
                    0,
                    "{case}"
                );
            }
            let state: String = sqlx::query_scalar("SELECT state FROM mounts WHERE lease_id = ?")
                .bind(&lease)
                .fetch_one(&journal)
                .await
                .unwrap();
            assert_ne!(state, "released", "{case}");
        }
    }

    #[tokio::test]
    async fn recorded_owner_cannot_mask_unowned_or_ambiguous_inventory_drift() {
        for case in [
            "unowned_snapshot",
            "foreign_uid",
            "different_device",
            "unmanaged_root",
            "unknown_state",
            "duplicate_uuid",
            "different_baseline",
            "multiple_owners",
        ] {
            let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
            init_journal(&journal).await.unwrap();
            let lease = Uuid::new_v4().to_string();
            let root = expected_mount_root(501, &lease).unwrap();
            let volume = "B8A38394-49AE-42F9-85FB-14DD9EA549FA";
            let owned = MountSnapshotRef {
                device_identifier: "disk5s1".into(),
                uuid: Uuid::new_v4().to_string(),
            };
            let manifest = serde_json::to_string(&vec![owned.clone()]).unwrap();
            sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?,501,?,'disk5s1','','','[]','[]',1,'released',datetime('now'),datetime('now'))")
                .bind(Uuid::new_v4().to_string()).bind(volume).execute(&journal).await.unwrap();
            sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?,501,?,'disk5s1','com.apple.TimeMachine.synthetic',?,?,'[]',1,'cleanup_pending',datetime('now'),datetime('now'))")
                .bind(&lease).bind(volume).bind(root.to_str().unwrap()).bind(&manifest).execute(&journal).await.unwrap();
            let fixture = FailedMountFixture {
                current: std::sync::Mutex::new(vec![owned.clone()]),
                root_safe: true,
                ..Default::default()
            };
            let valid = classify_status_rows_with_observation(&journal, None, None, &fixture)
                .await
                .unwrap();
            assert_eq!(valid.inventory_drift_mounts, 0);
            assert_eq!(valid.active_mounts, 1);
            match case {
                "unowned_snapshot" => fixture.current.lock().unwrap().push(MountSnapshotRef {
                    device_identifier: "disk5s1".into(),
                    uuid: Uuid::new_v4().to_string(),
                }),
                "foreign_uid" => {
                    sqlx::query("UPDATE mounts SET uid = 502 WHERE lease_id = ?")
                        .bind(&lease)
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "different_device" => {
                    sqlx::query(
                        "UPDATE mounts SET device_identifier = 'disk6s1' WHERE lease_id = ?",
                    )
                    .bind(&lease)
                    .execute(&journal)
                    .await
                    .unwrap();
                }
                "unmanaged_root" => {
                    sqlx::query("UPDATE mounts SET mount_root = '/unmanaged' WHERE lease_id = ?")
                        .bind(&lease)
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "unknown_state" => {
                    sqlx::query("UPDATE mounts SET state = 'unknown' WHERE lease_id = ?")
                        .bind(&lease)
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "duplicate_uuid" => {
                    let duplicate = serde_json::to_string(&[owned.clone(), owned.clone()]).unwrap();
                    sqlx::query("UPDATE mounts SET snapshot_manifest = ? WHERE lease_id = ?")
                        .bind(duplicate)
                        .bind(&lease)
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "different_baseline" => {
                    sqlx::query("UPDATE mounts SET reservation_manifest = ? WHERE lease_id = ?")
                        .bind(&manifest)
                        .bind(&lease)
                        .execute(&journal)
                        .await
                        .unwrap();
                }
                "multiple_owners" => {
                    sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at, last_renewed_at) VALUES (?,502,?,'disk5s1','','','[]','reserved',datetime('now'),datetime('now'))").bind(Uuid::new_v4().to_string()).bind(volume).execute(&journal).await.unwrap();
                }
                _ => unreachable!(),
            }
            let blocked = classify_status_rows_with_observation(&journal, None, None, &fixture)
                .await
                .unwrap();
            assert_eq!(
                blocked.inventory_drift_mounts, 1,
                "{case}: preserve the released row's independent drift blocker"
            );
            assert_eq!(fixture.deletes.load(std::sync::atomic::Ordering::SeqCst), 0);
        }
    }

    #[tokio::test]
    #[ignore = "explicit administrator-authorized read-only failed-mount diagnostic"]
    async fn readonly_installed_denied_mount_cleanup_audit() {
        let uid: u32 = std::env::var("TELEVYBACKUP_AUDIT_UID")
            .unwrap()
            .parse()
            .unwrap();
        assert_ne!(uid, 0);
        let expected: u32 = std::env::var("TELEVYBACKUP_AUDIT_EXPECTED")
            .unwrap()
            .parse()
            .unwrap();
        let options = SqliteConnectOptions::new()
            .filename(DEFAULT_MOUNT_HELPER_JOURNAL)
            .read_only(true)
            .create_if_missing(false);
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect_with(options)
            .await
            .unwrap();
        sqlx::query("PRAGMA query_only = ON")
            .execute(&pool)
            .await
            .unwrap();
        let classification = classify_status_rows(&pool, None, None).await.unwrap();
        println!(
            "readonly_candidate_status live={} pending={} unknown={} drift={} aggregate={}",
            classification.live_mounts,
            classification.pending_cleanup,
            classification.unknown_blocking_mounts,
            classification.inventory_drift_mounts,
            classification.active_mounts
        );
        assert_eq!(classification.active_mounts, expected);
        assert_eq!(classification.pending_cleanup, expected);
        assert_eq!(classification.live_mounts, 0);
        assert_eq!(classification.unknown_blocking_mounts, 0);
        assert_eq!(classification.inventory_drift_mounts, 0);
        let mut verified = 0;
        for candidate in classification.pending_cleanup_leases {
            let row = sqlx::query("SELECT uid, volume_uuid, device_identifier, snapshot_name, mount_root, mount_device, snapshot_manifest, reservation_manifest FROM mounts WHERE lease_id = ?")
                .bind(&candidate.lease_id).fetch_one(&pool).await.unwrap();
            assert_eq!(row.get::<i64, _>("uid"), i64::from(uid));
            assert_eq!(row.get::<i64, _>("mount_device"), 0);
            let root = PathBuf::from(row.get::<String, _>("mount_root"));
            assert_eq!(
                expected_mount_root(i64::from(uid), &candidate.lease_id).unwrap(),
                root
            );
            let volume: String = row.get("volume_uuid");
            let device: String = row.get("device_identifier");
            let name: String = row.get("snapshot_name");
            assert!(SystemCleanupObservation.unmounted_root(&root, &name, &device, &volume));
            let mut expected_inventory =
                parse_reservation_manifest(&row.get::<String, _>("reservation_manifest")).unwrap();
            expected_inventory.extend(
                parse_journal_manifest(&row.get::<String, _>("snapshot_manifest")).unwrap(),
            );
            assert!(manifests_equal(
                &snapshot_inventory_manifest(&device).unwrap(),
                &expected_inventory
            ));
            verified += 1;
        }
        println!("readonly_unmounted_cleanup_candidates_verified={verified}; mutations=0");
        assert_eq!(verified, expected);
    }

    struct GoneLegacySnapshots;

    impl LegacyRepairObservation for GoneLegacySnapshots {
        fn binding_valid(&self, _: &str, _: &str) -> bool {
            true
        }
        fn resolve_device(&self, _: &str, _: &str) -> Result<String, MountHelperError> {
            Ok("disk5s1".into())
        }

        fn inventory(&self, _: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
            Ok(Vec::new())
        }
        fn historical_root_unmounted(
            &self,
            _: &Path,
            _: u32,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<bool, MountHelperError> {
            Ok(true)
        }
    }

    #[derive(Default)]
    struct HistoricalObservation {
        inventory: Vec<MountSnapshotRef>,
        mounted: bool,
        change_device: bool,
        change_inventory: bool,
        inventory_calls: std::sync::atomic::AtomicUsize,
        device_calls: std::sync::atomic::AtomicUsize,
    }

    impl LegacyRepairObservation for HistoricalObservation {
        fn binding_valid(&self, _: &str, _: &str) -> bool {
            true
        }
        fn resolve_device(&self, _: &str, _: &str) -> Result<String, MountHelperError> {
            let call = self
                .device_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(if self.change_device && call > 0 {
                "disk6s1"
            } else {
                "disk5s1"
            }
            .into())
        }
        fn inventory(&self, _: &str) -> Result<Vec<MountSnapshotRef>, MountHelperError> {
            let call = self
                .inventory_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.change_inventory && call > 0 {
                return Ok(vec![MountSnapshotRef {
                    device_identifier: "disk5s1".into(),
                    uuid: "D8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
                }]);
            }
            Ok(self.inventory.clone())
        }
        fn historical_root_unmounted(
            &self,
            _: &Path,
            _: u32,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<bool, MountHelperError> {
            Ok(!self.mounted)
        }
    }

    async fn historical_row_fixture() -> (SqlitePool, String) {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let lease = Uuid::new_v4().to_string();
        let root = format!("/Users/synthetic/data/snapshot-access/mounts/501/{lease}");
        let manifest = serde_json::json!([{"device_identifier":"disk5s1", "uuid":"C8A38394-49AE-42F9-85FB-14DD9EA549FA"}]).to_string();
        sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, mount_device, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?, 501, 'B8A38394-49AE-42F9-85FB-14DD9EA549FA', 'disk5s1', 'com.apple.TimeMachine.synthetic', ?, 0, ?, '[]', 0, 'released', datetime('now', '-1 day'), datetime('now', '-1 day'))")
            .bind(&lease).bind(root).bind(manifest).execute(&journal).await.unwrap();
        (journal, lease)
    }

    #[tokio::test]
    async fn historical_retirement_rejects_present_snapshots_mounts_and_observation_races() {
        for observation in [
            HistoricalObservation {
                inventory: vec![MountSnapshotRef {
                    device_identifier: "disk5s1".into(),
                    uuid: "C8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
                }],
                ..Default::default()
            },
            HistoricalObservation {
                mounted: true,
                ..Default::default()
            },
            HistoricalObservation {
                change_device: true,
                ..Default::default()
            },
            HistoricalObservation {
                change_inventory: true,
                ..Default::default()
            },
        ] {
            let (journal, lease) = historical_row_fixture().await;
            let before: (String, String, String, i64) = sqlx::query_as("SELECT mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid FROM mounts WHERE lease_id = ?").bind(&lease).fetch_one(&journal).await.unwrap();
            let result = repair_legacy_with_observation(501, 501, &journal, &observation)
                .await
                .unwrap();
            assert_eq!(result.released, 0);
            assert_eq!(result.unknown, 1);
            let after = sqlx::query_as("SELECT mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid FROM mounts WHERE lease_id = ?").bind(&lease).fetch_one(&journal).await.unwrap();
            assert_eq!(before, after, "a rejected proof cannot change the journal");
        }
    }

    #[tokio::test]
    async fn historical_retirement_preserves_live_foreign_recent_and_corrupt_records() {
        for mutation in [
            "UPDATE mounts SET uid = 502",
            "UPDATE mounts SET state = 'active'",
            "UPDATE mounts SET last_renewed_at = datetime('now')",
            "UPDATE mounts SET reservation_manifest_valid = 1, reservation_manifest = 'invalid'",
            "UPDATE mounts SET mount_device = 42",
        ] {
            let (journal, lease) = historical_row_fixture().await;
            sqlx::query(mutation).execute(&journal).await.unwrap();
            let before: (String, String, String, i64) = sqlx::query_as("SELECT mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid FROM mounts WHERE lease_id = ?").bind(&lease).fetch_one(&journal).await.unwrap();
            let result = repair_legacy_with_observation(501, 501, &journal, &GoneLegacySnapshots)
                .await
                .unwrap();
            assert_eq!(result.released, 0, "{mutation}");
            let after = sqlx::query_as("SELECT mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid FROM mounts WHERE lease_id = ?").bind(&lease).fetch_one(&journal).await.unwrap();
            assert_eq!(before, after);
        }
    }

    #[tokio::test]
    async fn historical_retirement_preserves_user_directory_and_unrecorded_inventory() {
        let (journal, lease) = historical_row_fixture().await;
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary
            .path()
            .join("snapshot-access/mounts/501")
            .join(&lease);
        fs::create_dir_all(&root).unwrap();
        let sentinel = root.join("synthetic-sentinel");
        fs::write(&sentinel, b"unchanged").unwrap();
        sqlx::query("UPDATE mounts SET mount_root = ?")
            .bind(root.to_str().unwrap())
            .execute(&journal)
            .await
            .unwrap();
        let unrecorded = MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: "D8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        };
        let observation = HistoricalObservation {
            inventory: vec![unrecorded.clone()],
            ..Default::default()
        };
        let result = repair_legacy_with_observation(501, 501, &journal, &observation)
            .await
            .unwrap();
        assert_eq!(result.released, 1);
        assert_eq!(result.remaining, 0);
        assert_eq!(fs::read(&sentinel).unwrap(), b"unchanged");
        let baseline: String =
            sqlx::query_scalar("SELECT reservation_manifest FROM mounts WHERE lease_id = ?")
                .bind(&lease)
                .fetch_one(&journal)
                .await
                .unwrap();
        assert!(manifests_equal(
            &parse_reservation_manifest(&baseline).unwrap(),
            &[unrecorded]
        ));
        let again = repair_legacy_with_observation(501, 501, &journal, &observation)
            .await
            .unwrap();
        assert_eq!(again.audited, 0);
        assert_eq!(again.remaining, 0);
    }

    #[tokio::test]
    async fn historical_retirement_cannot_bypass_another_volume_owner() {
        let (journal, lease) = historical_row_fixture().await;
        sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at, last_renewed_at) SELECT ?, 502, volume_uuid, device_identifier, '', '', '[]', 'reserved', datetime('now'), datetime('now') FROM mounts WHERE lease_id = ?")
            .bind(Uuid::new_v4().to_string()).bind(&lease).execute(&journal).await.unwrap();
        let result = repair_legacy_with_observation(501, 501, &journal, &GoneLegacySnapshots)
            .await
            .unwrap();
        assert_eq!(result.released, 0);
        let retained: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mounts WHERE lease_id = ? AND mount_root != '' AND reservation_manifest_valid = 0").bind(&lease).fetch_one(&journal).await.unwrap();
        assert_eq!(retained, 1);
    }

    #[test]
    fn historical_mount_observation_covers_sources_descendants_and_data_firmlinks() {
        let lease = "A8A38394-49AE-42F9-85FB-14DD9EA549FA";
        let root = PathBuf::from(format!(
            "/Users/synthetic/data/snapshot-access/mounts/501/{lease}"
        ));
        assert!(historical_root_layout(&root, 501, lease));
        assert!(!historical_root_layout(&root, 502, lease));
        assert!(!historical_root_layout(
            Path::new("/tmp/arbitrary"),
            501,
            lease
        ));
        for mount in [
            ObservedMount {
                source: "name@/dev/disk5s1".into(),
                root: "/unrelated/location".into(),
            },
            ObservedMount {
                source: "other".into(),
                root: root.join("nested"),
            },
            ObservedMount {
                source: "other".into(),
                root: PathBuf::from("/System/Volumes/Data").join(root.strip_prefix("/").unwrap()),
            },
        ] {
            assert!(mounted_legacy_source_or_root(
                &[mount],
                &root,
                "name",
                "disk5s1"
            ));
        }
        assert!(!mounted_legacy_source_or_root(
            &[ObservedMount {
                source: "other".into(),
                root: PathBuf::from(format!("{}-other", root.display()))
            }],
            &root,
            "name",
            "disk5s1"
        ));
    }

    #[tokio::test]
    async fn historical_user_roots_do_not_permanently_block_released_gone_snapshots() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        for index in 0..64 {
            let lease = Uuid::new_v4().to_string();
            let data_root = if index < 61 {
                "/Users/synthetic/Library/Application Support/TelevyBackup"
            } else {
                "/Users/synthetic/.codex/worktrees/synthetic/televy-backup/.dev/data"
            };
            let root = format!("{data_root}/snapshot-access/mounts/501/{lease}");
            let manifest = serde_json::json!([{
                "device_identifier": "disk5s1", "uuid": Uuid::new_v4().to_string()
            }])
            .to_string();
            sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, mount_device, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?, 501, 'B8A38394-49AE-42F9-85FB-14DD9EA549FA', 'disk5s1', 'com.apple.TimeMachine.synthetic', ?, 0, ?, '[]', 0, 'released', datetime('now', '-1 day'), datetime('now', '-1 day'))")
                .bind(&lease).bind(&root).bind(&manifest).execute(&journal).await.unwrap();
        }
        let result = repair_legacy_with_observation(501, 501, &journal, &GoneLegacySnapshots)
            .await
            .unwrap();
        assert_eq!(result.audited, 64);
        assert_eq!(
            result.released, 64,
            "released snapshots are gone: retire their journal identity without touching user directories"
        );
        assert_eq!(result.remaining, 0);
        assert_eq!(result.unknown, 0);
        let uncleared: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mounts WHERE mount_root != '' OR snapshot_manifest != '[]' OR reservation_manifest_valid != 1 OR state != 'released'").fetch_one(&journal).await.unwrap();
        assert_eq!(uncleared, 0);
    }

    #[tokio::test]
    async fn legacy_repair_audits_incomplete_released_rows_but_preserves_foreign_owners() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        for uid in [501_i64, 502] {
            sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, state, created_at, last_renewed_at) VALUES (?, ?, 'invalid-volume', 'disk5s1', '', '/untrusted', '[]', 'released', datetime('now', '-1 day'), datetime('now', '-1 day'))")
                .bind(Uuid::new_v4().to_string()).bind(uid).execute(&journal).await.unwrap();
        }
        let result = repair_legacy_request(501, 501, &journal).await.unwrap();
        assert_eq!(
            result.audited, 1,
            "incomplete released rows must reach the repair safety checks"
        );
        assert_eq!(result.blocked, 1);
        assert_eq!(result.repaired, 0);
        let unchanged: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM mounts WHERE state = 'released' AND mount_root = '/untrusted'",
        )
        .fetch_one(&journal)
        .await
        .unwrap();
        assert_eq!(unchanged, 2);
    }

    #[tokio::test]
    async fn legacy_repair_rejects_a_peer_uid_mismatch_before_touching_the_journal() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let error = repair_legacy_request(502, 501, &journal).await.unwrap_err();
        assert!(error.to_string().contains("UID does not match peer"));
        let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM mounts")
            .fetch_one(&journal)
            .await
            .unwrap();
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn legacy_repair_leaves_an_untrusted_mount_root_blocking() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let lease_id = "A8A38394-49AE-42F9-85FB-14DD9EA549FA";
        let volume_uuid = "B8A38394-49AE-42F9-85FB-14DD9EA549FA";
        let manifest = serde_json::json!([
            {"device_identifier": "disk5s1", "uuid": "C8A38394-49AE-42F9-85FB-14DD9EA549FA"}
        ])
        .to_string();
        let reservation = serde_json::json!([]).to_string();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, source_path, snapshot_name, mount_root, mount_device, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?, ?, ?, ?, '', '', ?, 0, ?, ?, 1, 'cleanup_pending', datetime('now', '-1 day'), datetime('now', '-1 day'))",
        )
        .bind(lease_id)
        .bind(501_i64)
        .bind(volume_uuid)
        .bind("disk5s1")
        .bind("/tmp/user-controlled-mount-root")
        .bind(&manifest)
        .bind(&reservation)
        .execute(&journal)
        .await
        .unwrap();

        let result = repair_legacy_request(501, 501, &journal).await.unwrap();
        assert_eq!(result.audited, 1);
        assert_eq!(result.repaired, 0);
        assert_eq!(result.unknown, 1);
        let row =
            sqlx::query("SELECT mount_device, state, mount_root FROM mounts WHERE lease_id = ?")
                .bind(lease_id)
                .fetch_one(&journal)
                .await
                .unwrap();
        assert_eq!(row.try_get::<i64, _>("mount_device").unwrap(), 0);
        assert_eq!(
            row.try_get::<String, _>("state").unwrap(),
            "cleanup_pending"
        );
        assert_eq!(
            row.try_get::<String, _>("mount_root").unwrap(),
            "/tmp/user-controlled-mount-root"
        );
    }

    #[test]
    fn legacy_repair_is_an_explicit_wire_operation() {
        let request = MountRequest {
            version: MOUNT_HELPER_PROTOCOL_VERSION,
            request_id: "legacy-repair".into(),
            method: MountMethod::RepairLegacy { uid: 501 },
        };
        let encoded = serde_json::to_string(&request).unwrap();
        let decoded: MountRequest = serde_json::from_str(&encoded).unwrap();
        assert!(matches!(
            decoded.method,
            MountMethod::RepairLegacy { uid: 501 }
        ));
        assert!(validate_mount_request(&decoded).is_ok());
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
    async fn recovery_preserves_an_unrecognized_state() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let lease_id = "A8A38394-49AE-42F9-85FB-14DD9EA549FA";
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at) VALUES (?, ?, ?, ?, '', '', '[]', '[]', 1, 'future_state', datetime('now'))",
        )
        .bind(lease_id)
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .execute(&journal)
        .await
        .unwrap();

        recover_journal(&journal).await.unwrap();

        let state: String = sqlx::query_scalar("SELECT state FROM mounts WHERE lease_id = ?")
            .bind(lease_id)
            .fetch_one(&journal)
            .await
            .unwrap();
        assert_eq!(state, "future_state");
    }

    #[tokio::test]
    async fn ordinary_release_does_not_rebaseline_a_released_tombstone() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let lease_id = "A8A38394-49AE-42F9-85FB-14DD9EA549FA";
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at) VALUES (?, ?, ?, ?, '', '', '[]', '[]', 1, 'released', datetime('now'))",
        )
        .bind(lease_id)
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .execute(&journal)
        .await
        .unwrap();

        let result = release_request(501, 501, lease_id, false, &journal)
            .await
            .unwrap();
        assert_eq!(result.cleanup_state, "reconcile_required");
    }

    #[test]
    fn legacy_repair_distinguishes_absent_and_corrupt_baselines() {
        assert!(legacy_reservation_baseline("[]", 0).unwrap().is_none());
        assert!(
            legacy_reservation_baseline("[]", 1)
                .unwrap()
                .is_some_and(|rows| rows.is_empty())
        );
        assert!(legacy_reservation_baseline("not-json", 1).is_err());
        assert!(legacy_reservation_baseline("[]", 2).is_err());
    }

    #[tokio::test]
    async fn released_tombstone_transition_converges_after_snapshot_deletion() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let lease_id = "A8A38394-49AE-42F9-85FB-14DD9EA549FA";
        let manifest =
            r#"[{"device_identifier":"disk5s1","uuid":"B8A38394-49AE-42F9-85FB-14DD9EA549FA"}]"#;
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at) VALUES (?, ?, ?, ?, '', '', ?, '[]', 1, 'cleanup_pending', datetime('now'))",
        )
        .bind(lease_id)
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .bind(manifest)
        .execute(&journal)
        .await
        .unwrap();

        assert_eq!(
            record_released_tombstone(&journal, lease_id, "[]", manifest).await,
            "released"
        );
        let state: String = sqlx::query_scalar("SELECT state FROM mounts WHERE lease_id = ?")
            .bind(lease_id)
            .fetch_one(&journal)
            .await
            .unwrap();
        let stored_manifest: String =
            sqlx::query_scalar("SELECT snapshot_manifest FROM mounts WHERE lease_id = ?")
                .bind(lease_id)
                .fetch_one(&journal)
                .await
                .unwrap();
        assert_eq!(state, "released");
        assert_eq!(stored_manifest, "[]");
    }

    #[tokio::test]
    async fn cleanup_without_recorded_mount_device_remains_blocked() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let temp = tempfile::tempdir().unwrap();
        let mount_root = temp.path().join("mount");
        fs::create_dir(&mount_root).unwrap();
        let result = cleanup_mount(
            &mount_root,
            &[],
            None,
            "C8A38394-49AE-42F9-85FB-14DD9EA549FA",
            &journal,
            "A8A38394-49AE-42F9-85FB-14DD9EA549FA",
            "disk5s1",
            "[]",
            "[]",
        )
        .await;
        assert_eq!(result, "cleanup_pending");
        assert!(mount_root.exists());
    }

    #[tokio::test]
    async fn status_keeps_an_unmaterialized_reserved_lease_unknown() {
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
        assert!(status.recovery_leases.is_empty());
        assert_eq!(status.unknown_blocking_mounts, Some(1));
    }

    #[tokio::test]
    async fn release_rejects_foreign_stale_reserved_rows_without_mutation() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        let lease = Uuid::new_v4().to_string();
        sqlx::query("INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?,502,'C8A38394-49AE-42F9-85FB-14DD9EA549FA','disk5s1','','','[]','[]',0,'reserved',datetime('now','-1 day'),datetime('now','-1 day'))")
            .bind(&lease).execute(&journal).await.unwrap();
        let before: String = sqlx::query_scalar("SELECT json_object('uid', uid, 'state', state, 'root', mount_root, 'device', mount_device, 'manifest', snapshot_manifest, 'baseline', reservation_manifest, 'valid', reservation_manifest_valid, 'renewed', last_renewed_at) FROM mounts WHERE lease_id = ?")
            .bind(&lease).fetch_one(&journal).await.unwrap();
        let fixture = FailedMountFixture::default();
        let result =
            release_request_with_observation(501, 501, &lease, false, &journal, &fixture).await;
        let after: String = sqlx::query_scalar("SELECT json_object('uid', uid, 'state', state, 'root', mount_root, 'device', mount_device, 'manifest', snapshot_manifest, 'baseline', reservation_manifest, 'valid', reservation_manifest_valid, 'renewed', last_renewed_at) FROM mounts WHERE lease_id = ?")
            .bind(&lease).fetch_one(&journal).await.unwrap();
        assert_eq!(before, after, "foreign release must not mutate the journal");
        assert!(
            matches!(result, Err(MountHelperError::Protocol(ref message)) if message == "mount lease belongs to another user")
        );
        assert_eq!(fixture.proofs.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert_eq!(
            fixture
                .inventory_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        assert_eq!(fixture.deletes.load(std::sync::atomic::Ordering::SeqCst), 0);

        // The owner still reaches the conservative invalid-baseline cleanup path.
        let owned = release_request_with_observation(502, 502, &lease, false, &journal, &fixture)
            .await
            .unwrap();
        assert_eq!(owned.cleanup_state, "cleanup_pending");
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
        assert!(status.recovery_leases.is_empty());
        assert_eq!(status.unknown_blocking_mounts, Some(1));
    }

    #[tokio::test]
    async fn status_exposes_an_age_gated_stale_active_lease_for_recovery() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?, ?, ?, ?, '', ?, ?, ?, 1, 'active', datetime('now', '-301 seconds'), datetime('now', '-301 seconds'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .bind("/private/var/tmp/televybackup-snapshot-mounts/501/A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(r#"[{"device_identifier":"disk5s1","uuid":"B8A38394-49AE-42F9-85FB-14DD9EA549FA"}]"#)
        .bind(r#"[{"device_identifier":"disk5s1","uuid":"B8A38394-49AE-42F9-85FB-14DD9EA549FA"}]"#)
        .execute(&journal)
        .await
        .unwrap();

        let status =
            status_for_volume_result(501, 501, "C8A38394-49AE-42F9-85FB-14DD9EA549FA", &journal)
                .await
                .unwrap();
        assert!(status.recovery_leases.is_empty());
        assert_eq!(status.unknown_blocking_mounts, Some(1));

        sqlx::query("UPDATE mounts SET mount_device = 1 WHERE lease_id = ?")
            .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
            .execute(&journal)
            .await
            .unwrap();
        let status =
            status_for_volume_result(501, 501, "C8A38394-49AE-42F9-85FB-14DD9EA549FA", &journal)
                .await
                .unwrap();
        assert_eq!(status.recovery_leases.len(), 1);
        assert_eq!(status.recovery_leases[0].state, "active");
    }

    #[tokio::test]
    async fn status_exposes_an_age_gated_reserved_lease_with_a_valid_baseline_for_recovery() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?, ?, ?, ?, '', '', '[]', ?, 1, 'reserved', datetime('now', '-301 seconds'), datetime('now', '-301 seconds'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FB")
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .bind(r#"[{"device_identifier":"disk5s1","uuid":"B8A38394-49AE-42F9-85FB-14DD9EA549FA"}]"#)
        .execute(&journal)
        .await
        .unwrap();

        let status =
            status_for_volume_result(501, 501, "C8A38394-49AE-42F9-85FB-14DD9EA549FA", &journal)
                .await
                .unwrap();
        assert_eq!(status.recovery_leases.len(), 1);
        assert_eq!(status.recovery_leases[0].state, "reserved");
        assert_eq!(status.recoverable_stale_mounts, Some(1));
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
    fn legacy_repair_migrates_missing_baseline_without_claiming_unrecorded_snapshots() {
        let recorded = vec![MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        }];
        let current = vec![
            recorded[0].clone(),
            MountSnapshotRef {
                device_identifier: "disk5s1".into(),
                uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
            },
        ];

        let (baseline, present_recorded) = infer_legacy_reservation_baseline(&current, &recorded);
        assert_eq!(baseline.len(), 1);
        assert_eq!(baseline[0].uuid, "B8A38394-49AE-42F9-85FB-14DD9EA549FA");
        assert_eq!(present_recorded.len(), 1);
        assert_eq!(
            present_recorded[0].device_identifier,
            recorded[0].device_identifier
        );
        assert_eq!(present_recorded[0].uuid, recorded[0].uuid);

        let (baseline, present_recorded) = infer_legacy_reservation_baseline(&[], &recorded);
        assert!(baseline.is_empty());
        assert!(present_recorded.is_empty());
    }

    #[test]
    fn legacy_repair_with_empty_recorded_manifest_keeps_the_current_inventory_as_baseline() {
        let current = vec![MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        }];
        let (baseline, release) = partition_legacy_manifests(&current, None, &[]).unwrap();
        assert_eq!(baseline.len(), 1);
        assert_eq!(baseline[0].uuid, current[0].uuid);
        assert!(release.is_empty());
    }

    #[test]
    fn legacy_repair_accepts_only_the_recorded_snapshot_mount_source() {
        assert!(legacy_mount_identity_matches(
            "snapshot-name@/dev/disk5s1",
            "snapshot-name",
            "disk5s1",
        ));
        assert!(!legacy_mount_identity_matches(
            "other-snapshot@/dev/disk5s1",
            "snapshot-name",
            "disk5s1",
        ));
        assert!(!legacy_mount_identity_matches(
            "/dev/disk9s1",
            "snapshot-name",
            "disk5s1",
        ));
    }

    #[test]
    fn legacy_repair_rebinds_snapshot_manifest_after_device_renumbering() {
        let current = vec![
            MountSnapshotRef {
                device_identifier: "disk3s1".into(),
                uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
            },
            MountSnapshotRef {
                device_identifier: "disk3s1".into(),
                uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
            },
        ];
        let historical_baseline = vec![MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: "B8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        }];
        let historical_recorded = vec![MountSnapshotRef {
            device_identifier: "disk5s1".into(),
            uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        }];

        let (baseline, release) =
            partition_legacy_manifests(&current, Some(&historical_baseline), &historical_recorded)
                .expect("unique snapshot UUIDs must be repairable");
        assert_eq!(baseline.len(), 1);
        assert_eq!(baseline[0].device_identifier, "disk3s1");
        assert_eq!(baseline[0].uuid, historical_baseline[0].uuid);
        assert_eq!(release.len(), 1);
        assert_eq!(release[0].device_identifier, "disk3s1");
        assert_eq!(release[0].uuid, historical_recorded[0].uuid);
    }

    #[test]
    fn duplicate_inventory_cannot_be_partitioned_for_deletion() {
        let snapshot = MountSnapshotRef {
            device_identifier: "disk3s1".into(),
            uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        };
        let mut duplicate = snapshot.clone();
        duplicate.uuid = duplicate.uuid.to_ascii_lowercase();
        let current = [snapshot.clone(), duplicate];
        assert!(partition_legacy_manifests(&current, None, &[snapshot]).is_none());
        assert!(partition_legacy_manifests(&current, None, &[]).is_none());
    }

    #[test]
    fn duplicate_inventory_and_reservation_are_rejected() {
        let uuid = "A8A38394-49AE-42F9-85FB-14DD9EA549FA";
        let plist = format!(
            "<plist version=\"1.0\"><dict><key>Snapshots</key><array><dict><key>SnapshotUUID</key><string>{uuid}</string></dict><dict><key>SnapshotUUID</key><string>{}</string></dict></array></dict></plist>",
            uuid.to_ascii_lowercase()
        );
        assert!(parse_snapshot_inventory(plist.as_bytes(), "disk3s1").is_err());
        let manifest = serde_json::json!([
            {"device_identifier": "disk3s1", "uuid": uuid},
            {"device_identifier": "disk3s1", "uuid": uuid.to_ascii_lowercase()}
        ]);
        assert!(parse_reservation_manifest(&manifest.to_string()).is_err());
    }

    #[test]
    fn legacy_repair_rejects_duplicate_snapshot_uuid_across_manifests() {
        let snapshot = MountSnapshotRef {
            device_identifier: "disk3s1".into(),
            uuid: "A8A38394-49AE-42F9-85FB-14DD9EA549FA".into(),
        };
        let current = [snapshot.clone()];
        let baseline = [snapshot.clone()];
        let recorded = [snapshot];
        assert!(partition_legacy_manifests(&current, Some(&baseline), &recorded).is_none());
    }

    #[test]
    fn volume_identity_parser_requires_apfs_and_returns_device_uuid() {
        let plist = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>FilesystemName</key><string>APFS</string>
<key>DeviceIdentifier</key><string>disk3s1</string>
<key>APFSVolumeUUID</key><string>A8A38394-49AE-42F9-85FB-14DD9EA549FA</string>
</dict></plist>"#;
        let parsed = parse_volume_identity(plist).unwrap();
        assert_eq!(parsed.0, "disk3s1");
        assert_eq!(parsed.1, "A8A38394-49AE-42F9-85FB-14DD9EA549FA");
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
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, '[]', 1, 'cleanup_pending', datetime('now'))",
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
        assert_eq!(status.pending_cleanup, Some(1));
        assert_eq!(status.recoverable_stale_mounts, Some(0));
        assert_eq!(status.unknown_blocking_mounts, Some(0));
        assert!(status.recovery_leases.is_empty());
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
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, '[]', 1, 'cleanup_pending', datetime('now'))",
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
        assert_eq!(status.pending_cleanup, Some(1));
        assert_eq!(
            status.blocked_volume_uuids,
            vec!["C8A38394-49AE-42F9-85FB-14DD9EA549FA"]
        );
    }

    #[tokio::test]
    async fn status_keeps_malformed_pending_cleanup_as_unknown_blocker() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 0, 'cleanup_pending', datetime('now'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .bind("snapshot")
        .bind("")
        .bind("[]")
        .bind("not-json")
        .execute(&journal)
        .await
        .unwrap();

        let status = status_result(&journal, 501).await.unwrap();
        assert_eq!(status.active_mounts, 1);
        assert_eq!(status.pending_cleanup, Some(0));
        assert!(status.pending_cleanup_leases.is_empty());
        assert_eq!(status.unknown_blocking_mounts, Some(1));
    }

    #[tokio::test]
    async fn status_keeps_an_unrecognized_state_as_an_unknown_blocker() {
        let journal = SqlitePool::connect("sqlite::memory:").await.unwrap();
        init_journal(&journal).await.unwrap();
        sqlx::query(
            "INSERT INTO mounts (lease_id, uid, volume_uuid, device_identifier, snapshot_name, mount_root, snapshot_manifest, reservation_manifest, reservation_manifest_valid, state, created_at, last_renewed_at) VALUES (?, ?, ?, ?, '', '', '[]', '[]', 1, 'future_state', datetime('now'), datetime('now'))",
        )
        .bind("A8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind(501_i64)
        .bind("C8A38394-49AE-42F9-85FB-14DD9EA549FA")
        .bind("disk5s1")
        .execute(&journal)
        .await
        .unwrap();

        let status = status_result(&journal, 501).await.unwrap();
        assert_eq!(status.active_mounts, 1);
        assert_eq!(status.unknown_blocking_mounts, Some(1));
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
            pending_cleanup: Some(0),
            blocked_volume_uuids: vec![],
            recovery_leases: vec![],
            ..Default::default()
        };
        assert!(validate_status_result(&result).is_err());
    }

    #[test]
    fn mount_status_accepts_the_unchanged_v098_helper() {
        let result = MountStatusResult {
            helper_version: HISTORICAL_ROOT_MOUNT_HELPER_VERSION.into(),
            build_commit: None,
            active_mounts: 0,
            pending_cleanup: Some(0),
            blocked_volume_uuids: vec![],
            recovery_leases: vec![],
            ..Default::default()
        };
        assert!(validate_status_result(&result).is_ok());
    }

    #[test]
    fn mount_status_accepts_the_current_identity_bound_helper() {
        let result = MountStatusResult {
            helper_version: MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: None,
            active_mounts: 0,
            pending_cleanup: Some(0),
            blocked_volume_uuids: vec![],
            recovery_leases: vec![],
            ..Default::default()
        };
        assert!(validate_status_result(&result).is_ok());
    }
}
