use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use plist::Value;
use sqlx::{SqlitePool, sqlite::SqliteConnectOptions};
use televybackup_snapshot_helper::{
    DEFAULT_JOURNAL_PATH, DEFAULT_SOCKET_PATH, LeaseResult, MIN_FREE_BYTES, Method, ProbeResult,
    ReleaseResult, Request, Response, ResponseResult, StatusResult, validate_request,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

const HELPER_VERSION: &str = "0.1.0";

#[derive(Debug, thiserror::Error)]
enum HelperError {
    #[error("{0}")]
    Message(String),
    #[error("command {program} failed ({status}): {stderr}")]
    Command {
        program: String,
        status: String,
        stderr: String,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
struct Lease {
    lease_id: String,
    uid: u32,
    run_id: String,
    volume_uuid: String,
    device_identifier: String,
    snapshot_uuid: String,
    snapshot_name: String,
    mount_root: PathBuf,
    source_relative_path: PathBuf,
    source_mount_point: PathBuf,
}

#[derive(Clone)]
struct HelperState {
    journal: SqlitePool,
    leases: Arc<Mutex<HashMap<String, Lease>>>,
    socket_path: PathBuf,
    journal_path: PathBuf,
}

#[derive(Debug, Clone)]
struct VolumeInfo {
    uuid: String,
    device_identifier: String,
    mount_point: PathBuf,
    filesystem: String,
    free_bytes: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().any(|arg| arg == "--version" || arg == "-V") {
        println!(
            "televybackup-snapshot-helper {} ({})",
            option_env!("TELEVYBACKUP_BUILD_VERSION").unwrap_or(HELPER_VERSION),
            option_env!("TELEVYBACKUP_BUILD_COMMIT").unwrap_or("unknown")
        );
        return Ok(());
    }
    let socket_path = std::env::var_os("TELEVYBACKUP_SNAPSHOT_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SOCKET_PATH));
    let journal_path = std::env::var_os("TELEVYBACKUP_SNAPSHOT_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_JOURNAL_PATH));

    let parent = journal_path
        .parent()
        .ok_or_else(|| HelperError::Message("journal has no parent".to_string()))?;
    fs::create_dir_all(parent)?;
    fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    verify_secure_path(parent, true)?;

    let connect_options = SqliteConnectOptions::new()
        .filename(&journal_path)
        .create_if_missing(true);
    let journal = SqlitePool::connect_with(connect_options).await?;
    verify_root_owned_file(&journal_path)?;
    init_journal(&journal).await?;
    recover_journal(&journal).await?;

    if socket_path.exists() {
        verify_secure_path(&socket_path, false)?;
        fs::remove_file(&socket_path)?;
    }
    let socket_parent = socket_path
        .parent()
        .ok_or_else(|| HelperError::Message("socket has no parent".to_string()))?;
    fs::create_dir_all(socket_parent)?;
    verify_secure_path(socket_parent, true)?;
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o666))?;

    let state = HelperState {
        journal,
        leases: Arc::new(Mutex::new(HashMap::new())),
        socket_path,
        journal_path,
    };
    tracing_log_start(&state);

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, state).await {
                eprintln!("snapshot helper connection failed: {error}");
            }
        });
    }
}

fn verify_secure_path(path: &Path, directory: bool) -> Result<(), HelperError> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(HelperError::Message(
            "snapshot helper must run as root".into(),
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != 0 {
        return Err(HelperError::Message(format!(
            "unsafe snapshot helper asset owner: {}",
            path.display()
        )));
    }
    if (directory && !metadata.is_dir()) || (!directory && !metadata.file_type().is_socket()) {
        return Err(HelperError::Message(format!(
            "unsafe snapshot helper asset type: {}",
            path.display()
        )));
    }
    if directory && metadata.mode() & 0o022 != 0 {
        return Err(HelperError::Message(format!(
            "unsafe snapshot helper asset mode: {}",
            path.display()
        )));
    }
    Ok(())
}

fn verify_root_owned_file(path: &Path) -> Result<(), HelperError> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(HelperError::Message(
            "snapshot helper must run as root".into(),
        ));
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != 0 || !metadata.is_file() || metadata.mode() & 0o022 != 0 {
        return Err(HelperError::Message(format!(
            "unsafe snapshot helper journal ownership or mode: {}",
            path.display()
        )));
    }
    Ok(())
}

fn tracing_log_start(state: &HelperState) {
    eprintln!(
        "snapshot helper started socket={} journal={}",
        state.socket_path.display(),
        state.journal_path.display()
    );
}

async fn init_journal(pool: &SqlitePool) -> Result<(), HelperError> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS leases (
            lease_id TEXT PRIMARY KEY,
            uid INTEGER NOT NULL,
            run_id TEXT NOT NULL DEFAULT '',
            volume_uuid TEXT NOT NULL,
            device_identifier TEXT NOT NULL,
            snapshot_uuid TEXT NOT NULL,
            snapshot_name TEXT NOT NULL,
            mount_root TEXT NOT NULL,
            source_relative_path TEXT NOT NULL,
            source_mount_point TEXT NOT NULL,
            state TEXT NOT NULL,
            created_at TEXT NOT NULL
        )",
    )
    .execute(pool)
    .await?;
    // Existing journals predate run_id; keep them readable during an in-place update.
    let _ = sqlx::query("ALTER TABLE leases ADD COLUMN run_id TEXT NOT NULL DEFAULT ''")
        .execute(pool)
        .await;
    Ok(())
}

async fn recover_journal(pool: &SqlitePool) -> Result<(), HelperError> {
    let rows = sqlx::query("SELECT lease_id, mount_root, snapshot_uuid, device_identifier FROM leases WHERE state != 'released'")
        .fetch_all(pool)
        .await?;
    for row in rows {
        let lease_id: String = sqlx::Row::try_get(&row, "lease_id")?;
        let mount_root: String = sqlx::Row::try_get(&row, "mount_root")?;
        let snapshot_uuid: String = sqlx::Row::try_get(&row, "snapshot_uuid")?;
        let device_identifier: String = sqlx::Row::try_get(&row, "device_identifier")?;
        let unmount_ok = unmount_path(Path::new(&mount_root)).is_ok();
        let delete_ok = delete_snapshot(&device_identifier, &snapshot_uuid).is_ok();
        let state = if unmount_ok && delete_ok {
            "released"
        } else {
            "cleanup_pending"
        };
        sqlx::query("UPDATE leases SET state = ? WHERE lease_id = ?")
            .bind(state)
            .bind(lease_id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn serve_connection(stream: UnixStream, state: HelperState) -> Result<(), HelperError> {
    let uid = peer_uid(stream.as_raw_fd())?;
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = AsyncBufReader::new(read_half);
    let mut line = String::new();
    while reader.read_line(&mut line).await? > 0 {
        let request: Result<Request, _> = serde_json::from_str(line.trim());
        let response = match request {
            Ok(request) => handle_request(request, uid, &state).await,
            Err(error) => Response::error("unknown", "invalid_request", error.to_string()),
        };
        let encoded = serde_json::to_vec(&response)?;
        write_half.write_all(&encoded).await?;
        write_half.write_all(b"\n").await?;
        write_half.flush().await?;
        line.clear();
    }
    Ok(())
}

async fn handle_request(request: Request, uid: u32, state: &HelperState) -> Response {
    let request_id = request.request_id.clone();
    if let Err(error) = validate_request(&request) {
        return Response::error(request_id, "invalid_request", error);
    }
    let result = match request.method {
        Method::Status => status_result(state).await.map(ResponseResult::Status),
        Method::Probe {
            source_path,
            expected_volume_uuid,
        } => probe_source(Path::new(&source_path), expected_volume_uuid.as_deref())
            .map(ResponseResult::Probe),
        Method::AcquireLease {
            source_path,
            expected_volume_uuid,
            run_id,
        } => acquire_lease(
            uid,
            Path::new(&source_path),
            &expected_volume_uuid,
            &run_id,
            state,
        )
        .await
        .map(ResponseResult::Lease),
        Method::ReleaseLease { lease_id } => release_lease(uid, &lease_id, state)
            .await
            .map(ResponseResult::Released),
    };
    match result {
        Ok(result) => Response::ok(request_id, result),
        Err(error) => Response::error(request_id, error_code(&error), error.to_string()),
    }
}

fn error_code(error: &HelperError) -> &'static str {
    match error {
        HelperError::Message(message) if message.contains("unsupported") => "unsupported",
        HelperError::Message(message) if message.contains("lease") => "lease_conflict",
        HelperError::Command { .. } => "command_failed",
        HelperError::Io(_) => "io",
        HelperError::Sqlx(_) => "journal",
        HelperError::Json(_) => "protocol",
        HelperError::Message(_) => "invalid_request",
    }
}

async fn status_result(state: &HelperState) -> Result<StatusResult, HelperError> {
    let active_leases =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM leases WHERE state = 'active'")
            .fetch_one(&state.journal)
            .await? as u32;
    let pending_cleanup =
        sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM leases WHERE state = 'cleanup_pending'")
            .fetch_one(&state.journal)
            .await? as u32;
    Ok(StatusResult {
        active_leases,
        pending_cleanup,
        helper_version: HELPER_VERSION.to_string(),
    })
}

fn probe_source(path: &Path, expected_uuid: Option<&str>) -> Result<ProbeResult, HelperError> {
    let info = volume_info(path)?;
    let mut reason = None;
    if !info.filesystem.eq_ignore_ascii_case("APFS") {
        reason = Some("source volume is not APFS".to_string());
    } else if info.free_bytes < MIN_FREE_BYTES {
        reason = Some(format!(
            "APFS container has less than {MIN_FREE_BYTES} free bytes"
        ));
    } else if let Some(expected) = expected_uuid
        && !expected.eq_ignore_ascii_case(&info.uuid)
    {
        reason = Some("source volume UUID does not match the saved setting".to_string());
    }
    Ok(ProbeResult {
        volume_uuid: info.uuid,
        device_identifier: info.device_identifier,
        mount_point: info.mount_point.to_string_lossy().into_owned(),
        filesystem: info.filesystem,
        free_bytes: info.free_bytes,
        snapshot_supported: reason.is_none(),
        reason,
    })
}

async fn acquire_lease(
    uid: u32,
    source_path: &Path,
    expected_volume_uuid: &str,
    run_id: &str,
    state: &HelperState,
) -> Result<LeaseResult, HelperError> {
    let info = volume_info(source_path)?;
    if !info.uuid.eq_ignore_ascii_case(expected_volume_uuid) {
        return Err(HelperError::Message(
            "source volume UUID does not match lease request".into(),
        ));
    }
    if !info.filesystem.eq_ignore_ascii_case("APFS") {
        return Err(HelperError::Message("source volume is not APFS".into()));
    }
    if info.free_bytes < MIN_FREE_BYTES {
        return Err(HelperError::Message(format!(
            "APFS container has less than {MIN_FREE_BYTES} free bytes"
        )));
    }
    if let Some(nested_mount) = nested_mount_under(source_path, &info.mount_point)? {
        return Err(HelperError::Message(format!(
            "source contains nested mounted volume: {}",
            nested_mount.display()
        )));
    }
    {
        let leases = state
            .leases
            .lock()
            .map_err(|_| HelperError::Message("lease lock poisoned".into()))?;
        if leases
            .values()
            .any(|lease| lease.volume_uuid.eq_ignore_ascii_case(&info.uuid))
        {
            return Err(HelperError::Message(
                "volume already has an active lease".into(),
            ));
        }
    }
    let blocked: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM leases WHERE lower(volume_uuid) = lower(?) AND state IN ('active', 'cleanup_pending')",
    )
    .bind(&info.uuid)
    .fetch_one(&state.journal)
    .await?;
    if blocked > 0 {
        return Err(HelperError::Message(
            "volume has an active lease or pending snapshot cleanup".into(),
        ));
    }

    let before = snapshot_inventory_all(&info.device_identifier)?;
    run_command("/usr/bin/tmutil", ["localsnapshot"])?;
    let after = snapshot_inventory_all(&info.device_identifier)?;
    let new_snapshots: Vec<_> = after
        .into_iter()
        .filter_map(|(key, snapshot)| (!before.contains_key(&key)).then_some(snapshot))
        .collect();
    if new_snapshots.len() != 1 {
        return Err(HelperError::Message(
            "snapshot ownership could not be uniquely confirmed".into(),
        ));
    }
    let snapshot = &new_snapshots[0];
    let relative = source_path.strip_prefix(&info.mount_point).map_err(|_| {
        HelperError::Message("source path is outside its volume mount point".into())
    })?;
    let lease_id = Uuid::new_v4().to_string();
    let mount_root = PathBuf::from(format!(
        "/private/var/run/televybackup-snapshot/{uid}/{lease_id}"
    ));
    fs::create_dir_all(&mount_root)?;
    fs::set_permissions(&mount_root, fs::Permissions::from_mode(0o700))?;
    let mount_root_string = mount_root.to_string_lossy().into_owned();
    if let Err(error) = mount_snapshot(&snapshot.name, &info.mount_point, &mount_root) {
        let _ = fs::remove_dir(&mount_root);
        let _ = delete_snapshot(&info.device_identifier, &snapshot.uuid);
        return Err(error);
    }

    let lease = Lease {
        lease_id: lease_id.clone(),
        uid,
        run_id: run_id.to_string(),
        volume_uuid: info.uuid.clone(),
        device_identifier: info.device_identifier.clone(),
        snapshot_uuid: snapshot.uuid.clone(),
        snapshot_name: snapshot.name.clone(),
        mount_root: mount_root.clone(),
        source_relative_path: relative.to_path_buf(),
        source_mount_point: info.mount_point.clone(),
    };
    if let Err(error) = sqlx::query(
        "INSERT INTO leases (lease_id, uid, run_id, volume_uuid, device_identifier, snapshot_uuid, snapshot_name, mount_root, source_relative_path, source_mount_point, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', datetime('now'))",
    )
    .bind(&lease.lease_id)
    .bind(i64::from(uid))
    .bind(&lease.run_id)
    .bind(&lease.volume_uuid)
    .bind(&lease.device_identifier)
    .bind(&lease.snapshot_uuid)
    .bind(&lease.snapshot_name)
    .bind(lease.mount_root.to_string_lossy().as_ref())
    .bind(lease.source_relative_path.to_string_lossy().as_ref())
    .bind(lease.source_mount_point.to_string_lossy().as_ref())
    .execute(&state.journal)
    .await
    {
        let _ = unmount_path(&lease.mount_root);
        let _ = delete_snapshot(&lease.device_identifier, &lease.snapshot_uuid);
        return Err(HelperError::Sqlx(error));
    }
    state
        .leases
        .lock()
        .map_err(|_| HelperError::Message("lease lock poisoned".into()))?
        .insert(lease_id.clone(), lease);

    Ok(LeaseResult {
        lease_id,
        uid,
        volume_uuid: info.uuid,
        snapshot_uuid: snapshot.uuid.clone(),
        snapshot_name: snapshot.name.clone(),
        mount_root: mount_root_string,
        source_relative_path: relative.to_string_lossy().into_owned(),
        snapshot_created_at: snapshot.created_at.clone(),
    })
}

async fn release_lease(
    uid: u32,
    lease_id: &str,
    state: &HelperState,
) -> Result<ReleaseResult, HelperError> {
    let lease = {
        let mut leases = state
            .leases
            .lock()
            .map_err(|_| HelperError::Message("lease lock poisoned".into()))?;
        let lease = leases
            .remove(lease_id)
            .ok_or_else(|| HelperError::Message("lease not found".into()))?;
        if lease.uid != uid {
            leases.insert(lease_id.to_string(), lease);
            return Err(HelperError::Message("lease belongs to another user".into()));
        }
        lease
    };
    let unmount_result = unmount_path(&lease.mount_root);
    let delete_result = unmount_result
        .as_ref()
        .map(|_| delete_snapshot(&lease.device_identifier, &lease.snapshot_uuid));
    let cleanup_state = if unmount_result.is_ok() && delete_result.as_ref().is_ok_and(Result::is_ok)
    {
        let _ = fs::remove_dir(&lease.mount_root);
        "released"
    } else {
        "cleanup_pending"
    };
    sqlx::query("UPDATE leases SET state = ? WHERE lease_id = ?")
        .bind(cleanup_state)
        .bind(lease_id)
        .execute(&state.journal)
        .await?;
    if cleanup_state == "cleanup_pending" {
        return Ok(ReleaseResult {
            lease_id: lease_id.to_string(),
            cleanup_state: cleanup_state.to_string(),
        });
    }
    Ok(ReleaseResult {
        lease_id: lease_id.to_string(),
        cleanup_state: cleanup_state.to_string(),
    })
}

#[derive(Debug, Clone)]
struct SnapshotInfo {
    uuid: String,
    name: String,
    created_at: String,
}

fn snapshot_inventory_all(
    target_device_identifier: &str,
) -> Result<BTreeMap<(String, String), SnapshotInfo>, HelperError> {
    let output = run_command_output("/usr/sbin/diskutil", ["apfs", "list", "-plist"])?;
    let value = Value::from_reader_xml(output.as_slice())
        .map_err(|error| HelperError::Message(format!("invalid diskutil APFS plist: {error}")))?;
    let mut devices = Vec::new();
    collect_apfs_devices(&value, &mut devices);
    devices.push(target_device_identifier.to_string());
    devices.sort();
    devices.dedup();

    let mut inventory = BTreeMap::new();
    for device in devices {
        for (uuid, snapshot) in snapshot_inventory(&device)? {
            inventory.insert((device.clone(), uuid), snapshot);
        }
    }
    Ok(inventory)
}

fn collect_apfs_devices(value: &Value, devices: &mut Vec<String>) {
    match value {
        Value::Dictionary(dict) => {
            if let Some(device) = dict.get("DeviceIdentifier").and_then(Value::as_string)
                && (dict.contains_key("APFSVolumeUUID")
                    || dict.contains_key("VolumeUUID")
                    || dict.contains_key("MountPoint"))
            {
                devices.push(device.to_string());
            }
            for child in dict.values() {
                collect_apfs_devices(child, devices);
            }
        }
        Value::Array(array) => {
            for child in array {
                collect_apfs_devices(child, devices);
            }
        }
        _ => {}
    }
}

fn snapshot_inventory(
    device_identifier: &str,
) -> Result<BTreeMap<String, SnapshotInfo>, HelperError> {
    let output = run_command_output(
        "/usr/sbin/diskutil",
        ["apfs", "listSnapshots", "-plist", device_identifier],
    )?;
    let value = Value::from_reader_xml(output.as_slice()).map_err(|error| {
        HelperError::Message(format!("invalid diskutil snapshot plist: {error}"))
    })?;
    let mut inventory = BTreeMap::new();
    let Some(array) = value
        .as_dictionary()
        .and_then(|root| root.get("Snapshots"))
        .and_then(Value::as_array)
    else {
        return Ok(inventory);
    };
    for snapshot in array {
        let Some(dict) = snapshot.as_dictionary() else {
            continue;
        };
        let Some(uuid) = dict.get("SnapshotUUID").and_then(Value::as_string) else {
            continue;
        };
        let name = dict
            .get("SnapshotName")
            .and_then(Value::as_string)
            .unwrap_or_default();
        let created_at = dict
            .get("SnapshotCreationTime")
            .and_then(Value::as_string)
            .unwrap_or_default();
        inventory.insert(
            uuid.to_string(),
            SnapshotInfo {
                uuid: uuid.to_string(),
                name: name.to_string(),
                created_at: created_at.to_string(),
            },
        );
    }
    Ok(inventory)
}

fn volume_info(path: &Path) -> Result<VolumeInfo, HelperError> {
    let output = run_command_output(
        "/usr/sbin/diskutil",
        ["info", "-plist", path.to_string_lossy().as_ref()],
    )?;
    let value = Value::from_reader_xml(output.as_slice())
        .map_err(|error| HelperError::Message(format!("invalid diskutil volume plist: {error}")))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| HelperError::Message("diskutil volume info is not a dictionary".into()))?;
    let get = |key: &str| dict.get(key).and_then(Value::as_string).map(str::to_string);
    let uuid = get("DiskUUID")
        .ok_or_else(|| HelperError::Message("diskutil did not return a volume UUID".into()))?;
    let device_identifier = get("DeviceIdentifier").ok_or_else(|| {
        HelperError::Message("diskutil did not return a device identifier".into())
    })?;
    let mount_point = get("MountPoint")
        .ok_or_else(|| HelperError::Message("source volume is not mounted".into()))?;
    let filesystem = get("FilesystemName").unwrap_or_default();
    let free_bytes = available_bytes(Path::new(&mount_point))?;
    Ok(VolumeInfo {
        uuid,
        device_identifier,
        mount_point: PathBuf::from(mount_point),
        filesystem,
        free_bytes,
    })
}

fn available_bytes(path: &Path) -> Result<u64, HelperError> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| HelperError::Message("invalid source path".into()))?;
    let mut stat = unsafe { std::mem::zeroed::<libc::statfs>() };
    let result = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if result != 0 {
        return Err(HelperError::Io(std::io::Error::last_os_error()));
    }
    Ok((stat.f_bavail as u64).saturating_mul(stat.f_bsize as u64))
}

fn nested_mount_under(
    source_path: &Path,
    source_mount: &Path,
) -> Result<Option<PathBuf>, HelperError> {
    let output = Command::new("/sbin/mount").output()?;
    if !output.status.success() {
        return Err(HelperError::Command {
            program: "/sbin/mount".into(),
            status: output.status.to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().into(),
        });
    }
    let source_mount = source_mount
        .canonicalize()
        .unwrap_or_else(|_| source_mount.to_path_buf());
    let source_path = source_path
        .canonicalize()
        .unwrap_or_else(|_| source_path.to_path_buf());
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((_, suffix)) = line.rsplit_once(" on ") else {
            continue;
        };
        let Some((mount_point, _)) = suffix.split_once(" (") else {
            continue;
        };
        let mount_point = PathBuf::from(mount_point);
        if mount_point == source_mount || mount_point == source_path {
            continue;
        }
        if mount_point.starts_with(&source_path) {
            return Ok(Some(mount_point));
        }
    }
    Ok(None)
}

fn mount_snapshot(
    snapshot_name: &str,
    source_mount: &Path,
    mount_root: &Path,
) -> Result<(), HelperError> {
    let source = source_mount.to_string_lossy();
    run_command(
        "/sbin/mount_apfs",
        [
            "-s",
            snapshot_name,
            source.as_ref(),
            mount_root.to_string_lossy().as_ref(),
        ],
    )
}

fn unmount_path(path: &Path) -> Result<(), HelperError> {
    run_command("/sbin/umount", ["-f", path.to_string_lossy().as_ref()])
}

fn delete_snapshot(device_identifier: &str, snapshot_uuid: &str) -> Result<(), HelperError> {
    run_command(
        "/usr/sbin/diskutil",
        [
            "apfs",
            "deleteSnapshot",
            device_identifier,
            "-uuid",
            snapshot_uuid,
            "-wait",
        ],
    )
}

fn run_command<const N: usize, S: AsRef<OsStr>>(
    program: &str,
    args: [S; N],
) -> Result<(), HelperError> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(HelperError::Command {
        program: program.to_string(),
        status: output.status.to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn run_command_output<const N: usize, S: AsRef<OsStr>>(
    program: &str,
    args: [S; N],
) -> Result<Vec<u8>, HelperError> {
    let output = Command::new(program).args(args).output()?;
    if output.status.success() {
        return Ok(output.stdout);
    }
    Err(HelperError::Command {
        program: program.to_string(),
        status: output.status.to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
    })
}

fn peer_uid(fd: RawFd) -> Result<u32, HelperError> {
    #[cfg(target_os = "macos")]
    {
        let mut euid = 0_u32;
        let mut egid = 0_u32;
        let result = unsafe { libc::getpeereid(fd, &mut euid, &mut egid) };
        if result != 0 {
            return Err(HelperError::Io(std::io::Error::last_os_error()));
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
            return Err(HelperError::Io(std::io::Error::last_os_error()));
        }
        Ok(credentials.uid)
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    let _ = fd;
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    Err(HelperError::Message(
        "peer UID lookup is unsupported on this platform".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_inventory_accepts_empty_diskutil_plist() {
        let value = Value::from_reader_xml(br#"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>Snapshots</key><array/></dict></plist>"#.as_slice()).unwrap();
        assert_eq!(
            value
                .as_dictionary()
                .unwrap()
                .get("Snapshots")
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn release_paths_are_private_to_the_lease_uid() {
        let lease = Lease {
            lease_id: "l".into(),
            uid: 10,
            run_id: "run".into(),
            volume_uuid: "v".into(),
            device_identifier: "d".into(),
            snapshot_uuid: "s".into(),
            snapshot_name: "n".into(),
            mount_root: PathBuf::from("/tmp/m"),
            source_relative_path: PathBuf::from("a"),
            source_mount_point: PathBuf::from("/tmp"),
        };
        assert_ne!(lease.uid, 11);
    }
}
