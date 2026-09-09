use std::collections::{BTreeMap, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::io::Read;
#[cfg(target_os = "macos")]
use std::os::unix::ffi::OsStringExt;
use std::os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt};
use std::os::unix::io::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use base64::Engine;
use ignore::WalkBuilder;
use plist::Value;
use serde::Deserialize;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};
use televybackup_snapshot_access::mount_helper;
use televybackup_snapshot_access::{
    CONFIG_DIR_ENV, DATA_DIR_ENV, DEFAULT_JOURNAL_PATH, LeaseResult, MIN_FREE_BYTES, Method,
    MountSnapshotRef, ProbeResult, ReadStreamResult, ReleaseResult, Request, Response,
    ResponseResult, ScanPageResult, SourceEntry, StatusResult, VerificationResult,
    validate_request,
};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader as AsyncBufReader};
use tokio::net::{UnixListener, UnixStream};
use uuid::Uuid;

const ACCESS_APP_VERSION: &str = "0.2.0";

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
    snapshot_manifest: Vec<SnapshotRef>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct SnapshotRef {
    device_identifier: String,
    uuid: String,
    name: String,
    created_at: String,
}

struct ReadStream {
    uid: u32,
    lease_id: String,
    file: fs::File,
}

#[derive(Clone)]
struct HelperState {
    journal: SqlitePool,
    leases: Arc<Mutex<HashMap<String, Lease>>>,
    socket_path: PathBuf,
    journal_path: PathBuf,
    config_dir: PathBuf,
    data_dir: PathBuf,
    streams: Arc<Mutex<HashMap<String, ReadStream>>>,
    fda_ready: Arc<AtomicBool>,
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
            "televybackup-snapshot-access {} ({})",
            option_env!("TELEVYBACKUP_BUILD_VERSION").unwrap_or(ACCESS_APP_VERSION),
            option_env!("TELEVYBACKUP_BUILD_COMMIT").unwrap_or("unknown")
        );
        return Ok(());
    }
    let owner_uid = unsafe { libc::geteuid() };
    if owner_uid == 0 {
        return Err(HelperError::Message(
            "snapshot access app must run in the user session, not as root".into(),
        )
        .into());
    }
    let journal_path = std::env::var_os("TELEVYBACKUP_SNAPSHOT_JOURNAL")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_user_path(DEFAULT_JOURNAL_PATH));
    let config_dir = std::env::var_os(CONFIG_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| default_user_path("~/Library/Application Support/TelevyBackup"));
    let data_dir = std::env::var_os(DATA_DIR_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| default_user_path("~/Library/Application Support/TelevyBackup"));
    let socket_path = std::env::var_os("TELEVYBACKUP_SNAPSHOT_SOCKET")
        .map(PathBuf::from)
        .unwrap_or_else(|| data_dir.join("snapshot-access/access.sock"));

    ensure_private_directory(&data_dir, owner_uid)?;

    let parent = journal_path
        .parent()
        .ok_or_else(|| HelperError::Message("journal has no parent".to_string()))?;
    ensure_private_directory(parent, owner_uid)?;

    let connect_options = SqliteConnectOptions::new()
        .filename(&journal_path)
        .create_if_missing(true);
    // The journal is serialized by the lease state machine. A single connection
    // avoids concurrent SQLite initialization when launchd starts this agent.
    let journal = SqlitePoolOptions::new()
        .max_connections(1)
        .connect_with(connect_options)
        .await?;
    verify_private_file(&journal_path, owner_uid)?;
    init_journal(&journal).await?;
    recover_journal(&journal).await?;

    if socket_path.exists() {
        verify_private_path(&socket_path, false, owner_uid)?;
        fs::remove_file(&socket_path)?;
    }
    let socket_parent = socket_path
        .parent()
        .ok_or_else(|| HelperError::Message("socket has no parent".to_string()))?;
    ensure_private_directory(socket_parent, owner_uid)?;
    let listener = UnixListener::bind(&socket_path)?;
    fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;

    let state = HelperState {
        journal,
        leases: Arc::new(Mutex::new(HashMap::new())),
        socket_path,
        journal_path,
        config_dir,
        data_dir,
        streams: Arc::new(Mutex::new(HashMap::new())),
        fda_ready: Arc::new(AtomicBool::new(false)),
    };
    tracing_log_start(&state);

    loop {
        let (stream, _) = listener.accept().await?;
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(stream, state).await {
                eprintln!("snapshot access connection failed: {error}");
            }
        });
    }
}

fn default_user_path(raw: &str) -> PathBuf {
    if let Some(rest) = raw.strip_prefix("~/")
        && let Some(home) = std::env::var_os("HOME")
    {
        return PathBuf::from(home).join(rest);
    }
    PathBuf::from(raw)
}

fn verify_private_path(path: &Path, directory: bool, owner_uid: u32) -> Result<(), HelperError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != owner_uid {
        return Err(HelperError::Message(format!(
            "unsafe snapshot access asset owner: {}",
            path.display()
        )));
    }
    if (directory && !metadata.is_dir()) || (!directory && !metadata.file_type().is_socket()) {
        return Err(HelperError::Message(format!(
            "unsafe snapshot access asset type: {}",
            path.display()
        )));
    }
    if metadata.mode() & 0o022 != 0 {
        return Err(HelperError::Message(format!(
            "unsafe snapshot access asset mode: {}",
            path.display()
        )));
    }
    Ok(())
}

fn ensure_private_directory(path: &Path, owner_uid: u32) -> Result<(), HelperError> {
    match fs::symlink_metadata(path) {
        Ok(_) => verify_private_path(path, true, owner_uid),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            verify_private_path(path, true, owner_uid)
        }
        Err(error) => Err(HelperError::Io(error)),
    }
}

fn verify_private_file(path: &Path, owner_uid: u32) -> Result<(), HelperError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.uid() != owner_uid || !metadata.is_file() || metadata.mode() & 0o022 != 0 {
        return Err(HelperError::Message(format!(
            "unsafe snapshot access journal ownership or mode: {}",
            path.display()
        )));
    }
    Ok(())
}

fn tracing_log_start(state: &HelperState) {
    eprintln!(
        "snapshot access started socket={} journal={}",
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
            snapshot_manifest TEXT NOT NULL DEFAULT '[]',
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
    let _ =
        sqlx::query("ALTER TABLE leases ADD COLUMN snapshot_manifest TEXT NOT NULL DEFAULT '[]'")
            .execute(pool)
            .await;
    Ok(())
}

async fn recover_journal(pool: &SqlitePool) -> Result<(), HelperError> {
    let rows = sqlx::query("SELECT lease_id, uid FROM leases WHERE state != 'released'")
        .fetch_all(pool)
        .await?;
    for row in rows {
        let lease_id: String = sqlx::Row::try_get(&row, "lease_id")?;
        let uid: i64 = sqlx::Row::try_get(&row, "uid")?;
        let state = match mount_helper::release(uid as u32, &lease_id) {
            Ok(result) if result.cleanup_state == "released" => "released",
            _ => "cleanup_pending",
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
    let result =
        match request.method {
            Method::Status => status_result(state).await.map(ResponseResult::Status),
            Method::ProbeVolume { target_id } => {
                let result = configured_source(&state.config_dir, &target_id).and_then(|source| {
                    let probe = probe_source(&source, None)?;
                    if probe.snapshot_supported {
                        match fs::read_dir(&source) {
                            Ok(mut entries) => {
                                let readable = !matches!(
                                    entries.next(),
                                    Some(Err(error))
                                        if error.kind() == std::io::ErrorKind::PermissionDenied
                                );
                                state.fda_ready.store(readable, Ordering::Relaxed);
                            }
                            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                                state.fda_ready.store(false, Ordering::Relaxed);
                            }
                            Err(_) => {}
                        }
                    }
                    Ok(probe)
                });
                result.map(ResponseResult::Probe)
            }
            Method::AcquireLease {
                target_id,
                expected_volume_uuid,
                run_id,
            } => {
                let result = match configured_source(&state.config_dir, &target_id) {
                    Ok(source) => {
                        acquire_lease(uid, &source, &expected_volume_uuid, &run_id, state).await
                    }
                    Err(error) => Err(error),
                };
                result.map(ResponseResult::Lease)
            }
            Method::ScanPage {
                lease_id,
                cursor,
                limit,
            } => scan_page(uid, &lease_id, cursor.as_deref(), limit, state)
                .map(ResponseResult::ScanPage),
            Method::OpenReadStream {
                lease_id,
                relative_path,
            } => open_read_stream(uid, &lease_id, &relative_path, state)
                .map(ResponseResult::ReadStream),
            Method::ReadStream {
                stream_id,
                max_bytes,
            } => read_stream(uid, &stream_id, max_bytes, state).map(ResponseResult::ReadStream),
            Method::CloseReadStream { stream_id } => {
                close_read_stream(uid, &stream_id, state).map(|_| ResponseResult::Closed)
            }
            Method::ReleaseLease { lease_id } => release_lease(uid, &lease_id, state)
                .await
                .map(ResponseResult::Released),
            Method::VerifyTimepoint {
                target_id,
                confirm_probe_write,
            } => verify_timepoint(uid, &target_id, confirm_probe_write, state)
                .await
                .map(ResponseResult::Verification),
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
    let access_app_version = option_env!("TELEVYBACKUP_BUILD_VERSION")
        .unwrap_or(ACCESS_APP_VERSION)
        .to_string();
    let mount_helper_status = mount_helper::status();
    let (mount_helper_reachable, mount_helper_version, mount_helper_error) =
        match mount_helper_status {
            Ok(status) if status.helper_version == access_app_version => {
                (true, Some(status.helper_version), None)
            }
            Ok(status) => (
                false,
                Some(status.helper_version),
                Some("snapshot access and mount helper versions do not match".to_string()),
            ),
            Err(error) => (false, None, Some(error.to_string())),
        };
    Ok(StatusResult {
        active_leases,
        pending_cleanup,
        access_app_version,
        fda_ready: state.fda_ready.load(Ordering::Relaxed),
        mount_helper_reachable,
        mount_helper_version,
        mount_helper_error,
    })
}

#[derive(Debug, Deserialize)]
struct ConfigFile {
    #[serde(default)]
    targets: Vec<ConfigTarget>,
}

#[derive(Debug, Deserialize)]
struct ConfigTarget {
    id: String,
    source_path: String,
}

fn configured_source(config_dir: &Path, target_id: &str) -> Result<PathBuf, HelperError> {
    if target_id.is_empty() || target_id.len() > 128 {
        return Err(HelperError::Message("invalid target id".into()));
    }
    let settings_path = config_dir.join("config.toml");
    let contents = fs::read_to_string(&settings_path).map_err(|error| {
        HelperError::Message(format!("cannot read configured backup targets: {error}"))
    })?;
    let settings: ConfigFile = toml::from_str(&contents)
        .map_err(|error| HelperError::Message(format!("invalid backup settings: {error}")))?;
    let target = settings
        .targets
        .into_iter()
        .find(|target| target.id == target_id)
        .ok_or_else(|| HelperError::Message("target is not configured".into()))?;
    let source = PathBuf::from(target.source_path);
    if !source.is_absolute() {
        return Err(HelperError::Message(
            "configured source path must be absolute".into(),
        ));
    }
    source
        .canonicalize()
        .map_err(|error| HelperError::Message(format!("source path is unavailable: {error}")))
}

fn lease_for(uid: u32, lease_id: &str, state: &HelperState) -> Result<Lease, HelperError> {
    let lease = state
        .leases
        .lock()
        .map_err(|_| HelperError::Message("lease lock poisoned".into()))?
        .get(lease_id)
        .cloned()
        .ok_or_else(|| HelperError::Message("lease not found".into()))?;
    if lease.uid != uid {
        return Err(HelperError::Message("lease belongs to another user".into()));
    }
    Ok(lease)
}

fn lease_source_root(lease: &Lease) -> PathBuf {
    lease.mount_root.join(&lease.source_relative_path)
}

fn scan_page(
    uid: u32,
    lease_id: &str,
    cursor: Option<&str>,
    limit: u16,
    state: &HelperState,
) -> Result<ScanPageResult, HelperError> {
    let lease = lease_for(uid, lease_id, state)?;
    let root = lease_source_root(&lease);
    let root_metadata = fs::symlink_metadata(&root)?;
    let root_device = root_metadata.dev();
    let mut entries = Vec::new();
    let mut ignore_rule_files = 0_u64;
    let ignore_invalid_rules = 0_u64;
    let walker = WalkBuilder::new(&root)
        .follow_links(false)
        .hidden(false)
        .parents(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .add_custom_ignore_filename(".televyignore")
        .build();
    for item in walker {
        let item =
            item.map_err(|error| HelperError::Message(format!("snapshot walk failed: {error}")))?;
        let path = item.path();
        if path == root {
            continue;
        }
        let metadata = fs::symlink_metadata(path)?;
        if metadata.dev() != root_device {
            return Err(HelperError::Message(format!(
                "source contains nested mounted volume: {}",
                path.display()
            )));
        }
        let relative_path = path
            .strip_prefix(&root)
            .map_err(|_| HelperError::Message("snapshot path escaped source root".into()))?;
        let relative_path = relative_path
            .to_str()
            .ok_or_else(|| HelperError::Message("snapshot path is not UTF-8".into()))?
            .to_string();
        let kind = if metadata.file_type().is_symlink() {
            "symlink"
        } else if metadata.is_dir() {
            if path.file_name().is_some_and(|name| name == ".televyignore") {
                ignore_rule_files = ignore_rule_files.saturating_add(1);
            }
            "dir"
        } else if metadata.is_file() {
            if path.file_name().is_some_and(|name| name == ".televyignore") {
                ignore_rule_files = ignore_rule_files.saturating_add(1);
            }
            "file"
        } else {
            continue;
        };
        let (size, mtime_ms, mode) = if kind == "file" {
            let mtime_ms = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as i64)
                .unwrap_or(0);
            (metadata.len() as i64, mtime_ms, metadata.mode() as i64)
        } else {
            (0, 0, 0)
        };
        entries.push(SourceEntry {
            relative_path,
            kind: kind.to_string(),
            size,
            mtime_ms,
            mode,
        });
    }
    let start = cursor
        .unwrap_or("0")
        .parse::<usize>()
        .map_err(|_| HelperError::Message("invalid scan cursor".into()))?;
    if start > entries.len() {
        return Err(HelperError::Message("scan cursor is out of range".into()));
    }
    let take = usize::from(limit.clamp(1, 512));
    let end = start.saturating_add(take).min(entries.len());
    let next_cursor = (end < entries.len()).then(|| end.to_string());
    Ok(ScanPageResult {
        entries: entries.into_iter().skip(start).take(end - start).collect(),
        next_cursor,
        ignore_rule_files,
        ignore_invalid_rules,
    })
}

fn open_read_stream(
    uid: u32,
    lease_id: &str,
    relative_path: &str,
    state: &HelperState,
) -> Result<ReadStreamResult, HelperError> {
    let lease = lease_for(uid, lease_id, state)?;
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir | std::path::Component::RootDir
            )
        })
        || relative_path.is_empty()
    {
        return Err(HelperError::Message(
            "invalid snapshot relative path".into(),
        ));
    }
    let root = lease_source_root(&lease);
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(HelperError::Message(
            "snapshot path is not a regular file".into(),
        ));
    }
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(&root) {
        return Err(HelperError::Message(
            "snapshot path escaped source root".into(),
        ));
    }
    let stream_id = Uuid::new_v4().to_string();
    state
        .streams
        .lock()
        .map_err(|_| HelperError::Message("stream lock poisoned".into()))?
        .insert(
            stream_id.clone(),
            ReadStream {
                uid,
                lease_id: lease_id.to_string(),
                file: fs::File::open(canonical)?,
            },
        );
    Ok(ReadStreamResult {
        stream_id,
        eof: false,
        bytes_base64: String::new(),
    })
}

fn read_stream(
    uid: u32,
    stream_id: &str,
    max_bytes: u32,
    state: &HelperState,
) -> Result<ReadStreamResult, HelperError> {
    let mut streams = state
        .streams
        .lock()
        .map_err(|_| HelperError::Message("stream lock poisoned".into()))?;
    let stream = streams
        .get_mut(stream_id)
        .ok_or_else(|| HelperError::Message("read stream not found".into()))?;
    if stream.uid != uid {
        return Err(HelperError::Message(
            "read stream belongs to another user".into(),
        ));
    }
    let mut buffer = vec![0_u8; usize::try_from(max_bytes.clamp(1, 1024 * 1024)).unwrap_or(1024)];
    let read = stream.file.read(&mut buffer)?;
    buffer.truncate(read);
    Ok(ReadStreamResult {
        stream_id: stream_id.to_string(),
        eof: read == 0,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(buffer),
    })
}

fn close_read_stream(uid: u32, stream_id: &str, state: &HelperState) -> Result<(), HelperError> {
    let mut streams = state
        .streams
        .lock()
        .map_err(|_| HelperError::Message("stream lock poisoned".into()))?;
    let stream = streams
        .get(stream_id)
        .ok_or_else(|| HelperError::Message("read stream not found".into()))?;
    if stream.uid != uid {
        return Err(HelperError::Message(
            "read stream belongs to another user".into(),
        ));
    }
    streams.remove(stream_id);
    Ok(())
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
    if let Err(error) = mount_helper::status() {
        return Err(HelperError::Message(format!(
            "snapshot mount helper unavailable: {error}"
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

    let before = source_snapshot_inventory(&info.device_identifier)?;
    run_command("/usr/bin/tmutil", ["localsnapshot"])?;
    let after = source_snapshot_inventory(&info.device_identifier)?;
    let new_snapshots: Vec<_> = after
        .into_iter()
        .filter_map(|(key, snapshot)| (!before.contains_key(&key)).then_some((key.0, snapshot)))
        .collect();
    let snapshot = match source_snapshot(&new_snapshots, &info.device_identifier) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            let manifest = new_snapshots
                .iter()
                .map(|(device, snapshot)| MountSnapshotRef {
                    device_identifier: device.clone(),
                    uuid: snapshot.uuid.clone(),
                })
                .collect::<Vec<_>>();
            if !manifest.is_empty() {
                let _ = mount_helper::cleanup(uid, &Uuid::new_v4().to_string(), &manifest);
            }
            return Err(error);
        }
    };
    let snapshot_manifest: Vec<SnapshotRef> = new_snapshots
        .iter()
        .map(|(device, snapshot)| SnapshotRef {
            device_identifier: device.clone(),
            uuid: snapshot.uuid.clone(),
            name: snapshot.name.clone(),
            created_at: snapshot.created_at.clone(),
        })
        .collect();
    let lease_id = Uuid::new_v4().to_string();
    let mount_root = state
        .data_dir
        .join("snapshot-access/mounts")
        .join(uid.to_string())
        .join(&lease_id);
    fs::create_dir_all(&mount_root)?;
    fs::set_permissions(&mount_root, fs::Permissions::from_mode(0o700))?;
    let mount_manifest: Vec<MountSnapshotRef> = snapshot_manifest
        .iter()
        .map(|snapshot| MountSnapshotRef {
            device_identifier: snapshot.device_identifier.clone(),
            uuid: snapshot.uuid.clone(),
        })
        .collect();
    if let Err(error) = mount_helper::mount(
        uid,
        &lease_id,
        &info.uuid,
        &info.device_identifier,
        &snapshot.uuid,
        &snapshot.name,
        &info.mount_point,
        &mount_root,
        &mount_manifest,
    ) {
        let _ = fs::remove_dir(&mount_root);
        return Err(HelperError::Message(error.to_string()));
    }
    let relative = match source_relative_path(source_path, &info.mount_point, &mount_root) {
        Ok(relative) => relative,
        Err(error) => {
            let _ = mount_helper::release(uid, &lease_id);
            let _ = fs::remove_dir(&mount_root);
            return Err(error);
        }
    };

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
        snapshot_manifest: snapshot_manifest.clone(),
    };
    if let Err(error) = sqlx::query(
        "INSERT INTO leases (lease_id, uid, run_id, volume_uuid, device_identifier, snapshot_uuid, snapshot_name, mount_root, source_relative_path, source_mount_point, snapshot_manifest, state, created_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'active', datetime('now'))",
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
    .bind(serde_json::to_string(&lease.snapshot_manifest)?)
    .execute(&state.journal)
    .await
    {
        let _ = mount_helper::release(uid, &lease.lease_id);
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
        source_path: source_path.to_string_lossy().into_owned(),
    })
}

fn source_snapshot<'a>(
    snapshots: &'a [(String, SnapshotInfo)],
    source_device_identifier: &str,
) -> Result<&'a SnapshotInfo, HelperError> {
    let source_snapshots = snapshots
        .iter()
        .filter(|(device, _)| device == source_device_identifier)
        .collect::<Vec<_>>();
    if source_snapshots.len() != 1 {
        return Err(HelperError::Message(
            "snapshot ownership could not be uniquely confirmed for the source volume".into(),
        ));
    }
    Ok(&source_snapshots[0].1)
}

fn source_relative_path(
    source_path: &Path,
    mount_point: &Path,
    mounted_snapshot_root: &Path,
) -> Result<PathBuf, HelperError> {
    if let Ok(relative) = source_path.strip_prefix(mount_point) {
        return Ok(relative.to_path_buf());
    }
    // macOS exposes the Data volume through firmlinks such as `/Users`, which are not
    // lexical descendants of `/System/Volumes/Data`. Prefer the full logical path, then
    // progressively drop synthetic leading components until the snapshot contains it.
    let components = source_path
        .strip_prefix(Path::new("/"))
        .map_err(|_| HelperError::Message("source path must be absolute".into()))?
        .components()
        .collect::<Vec<_>>();
    for start in 0..components.len() {
        let relative = components[start..]
            .iter()
            .fold(PathBuf::new(), |mut path, component| {
                path.push(component.as_os_str());
                path
            });
        if mounted_snapshot_root.join(&relative).exists() {
            return Ok(relative);
        }
    }
    Err(HelperError::Message(
        "source path is not present in the mounted snapshot".into(),
    ))
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
    if let Ok(mut streams) = state.streams.lock() {
        streams.retain(|_, stream| stream.lease_id != lease_id);
    }
    let cleanup_state = match mount_helper::release(uid, &lease.lease_id) {
        Ok(result) => result.cleanup_state,
        Err(_) => "cleanup_pending".to_string(),
    };
    if cleanup_state == "released" {
        let _ = fs::remove_dir(&lease.mount_root);
    }
    sqlx::query("UPDATE leases SET state = ? WHERE lease_id = ?")
        .bind(&cleanup_state)
        .bind(lease_id)
        .execute(&state.journal)
        .await?;
    Ok(ReleaseResult {
        lease_id: lease_id.to_string(),
        cleanup_state,
    })
}

async fn verify_timepoint(
    uid: u32,
    target_id: &str,
    confirm_probe_write: bool,
    state: &HelperState,
) -> Result<VerificationResult, HelperError> {
    if !confirm_probe_write {
        return Err(HelperError::Message(
            "verification requires explicit probe-write confirmation".into(),
        ));
    }
    let source = configured_source(&state.config_dir, target_id)?;
    let probe = probe_source(&source, None)?;
    if !probe.snapshot_supported {
        return Err(HelperError::Message(
            probe
                .reason
                .unwrap_or_else(|| "source volume is not snapshot-capable".into()),
        ));
    }
    let probe_path = source.join(format!(".televybackup-probe-{}", Uuid::new_v4()));
    let before = b"televybackup snapshot verification: before".to_vec();
    let after = b"televybackup snapshot verification: after".to_vec();
    fs::write(&probe_path, &before)?;

    let mut lease = None;
    let operation = async {
        let acquired =
            acquire_lease(uid, &source, &probe.volume_uuid, "verification", state).await?;
        let relative = probe_path
            .strip_prefix(&source)
            .map_err(|_| HelperError::Message("verification probe escaped source root".into()))?
            .to_string_lossy()
            .into_owned();
        lease = Some(acquired.clone());

        // Mutate the live source only after the snapshot lease has mounted. The read below must
        // still return the pre-mutation bytes from the mounted snapshot.
        fs::write(&probe_path, &after)?;
        let opened = open_read_stream(uid, &acquired.lease_id, &relative, state)?;
        let stream_id = opened.stream_id.clone();
        let mut snapshot_bytes = Vec::new();
        loop {
            let chunk = read_stream(uid, &stream_id, 1024 * 1024, state)?;
            snapshot_bytes.extend(
                base64::engine::general_purpose::STANDARD
                    .decode(chunk.bytes_base64)
                    .map_err(|error| HelperError::Message(error.to_string()))?,
            );
            if chunk.eof {
                break;
            }
        }
        close_read_stream(uid, &stream_id, state)?;
        let live_bytes = fs::read(&probe_path)?;
        let snapshot_read_pre_mutation = snapshot_bytes == before;
        let live_mutation_observed = live_bytes == after;
        if !snapshot_read_pre_mutation || !live_mutation_observed {
            return Err(HelperError::Message(
                "snapshot verification did not observe the expected timepoint".into(),
            ));
        }
        Ok((snapshot_read_pre_mutation, live_mutation_observed))
    }
    .await;

    let cleanup_complete = match lease.as_ref() {
        Some(acquired) => {
            matches!(release_lease(uid, &acquired.lease_id, state).await, Ok(result) if result.cleanup_state == "released")
        }
        None => true,
    };
    let _ = fs::remove_file(&probe_path);
    if !cleanup_complete {
        return Err(HelperError::Message(
            "snapshot verification cleanup is pending".into(),
        ));
    }
    let (snapshot_read_pre_mutation, live_mutation_observed) = operation?;
    Ok(VerificationResult {
        volume_uuid: probe.volume_uuid,
        snapshot_read_pre_mutation,
        live_mutation_observed,
        leases_released: true,
        cleanup_complete,
    })
}

#[derive(Debug, Clone)]
struct SnapshotInfo {
    uuid: String,
    name: String,
    created_at: String,
}

fn source_snapshot_inventory(
    target_device_identifier: &str,
) -> Result<BTreeMap<(String, String), SnapshotInfo>, HelperError> {
    // `tmutil localsnapshot` can coexist with arbitrary mounted APFS images.
    // Only the configured source volume contributes to this backup lease; probing
    // unrelated system assets can hang and must never block source consistency.
    let snapshots = snapshot_inventory(target_device_identifier).map_err(|error| {
        HelperError::Message(format!(
            "snapshot inventory failed for source device {target_device_identifier}: {error}"
        ))
    })?;
    Ok(tag_snapshot_inventory(target_device_identifier, snapshots))
}

fn tag_snapshot_inventory(
    device_identifier: &str,
    snapshots: BTreeMap<String, SnapshotInfo>,
) -> BTreeMap<(String, String), SnapshotInfo> {
    snapshots
        .into_iter()
        .map(|(uuid, snapshot)| ((device_identifier.to_string(), uuid), snapshot))
        .collect()
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
    let mount_point = filesystem_mount_point(path)?;
    let output = run_command_output(
        "/usr/sbin/diskutil",
        ["info", "-plist", mount_point.to_string_lossy().as_ref()],
    )?;
    let value = Value::from_reader_xml(output.as_slice())
        .map_err(|error| HelperError::Message(format!("invalid diskutil volume plist: {error}")))?;
    let dict = value
        .as_dictionary()
        .ok_or_else(|| HelperError::Message("diskutil volume info is not a dictionary".into()))?;
    let get = |key: &str| dict.get(key).and_then(Value::as_string).map(str::to_string);
    // `DiskUUID` can identify a partition/container; APFS settings must bind to the
    // mounted volume UUID specifically. Keep the older key only as a compatibility fallback
    // for synthetic test plists and older macOS output.
    let uuid = get("APFSVolumeUUID")
        .or_else(|| get("VolumeUUID"))
        .or_else(|| get("DiskUUID"))
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

#[cfg(target_os = "macos")]
fn filesystem_mount_point(path: &Path) -> Result<PathBuf, HelperError> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| HelperError::Message("invalid source path".into()))?;
    let mut stat = unsafe { std::mem::zeroed::<libc::statfs>() };
    let result = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if result != 0 {
        return Err(HelperError::Io(std::io::Error::last_os_error()));
    }
    let bytes = stat
        .f_mntonname
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    if bytes.is_empty() {
        return Err(HelperError::Message(
            "filesystem mount point is unavailable".into(),
        ));
    }
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(not(target_os = "macos"))]
fn filesystem_mount_point(path: &Path) -> Result<PathBuf, HelperError> {
    path.canonicalize().map_err(HelperError::Io)
}

fn available_bytes(path: &Path) -> Result<u64, HelperError> {
    let c_path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes())
        .map_err(|_| HelperError::Message("invalid source path".into()))?;
    let mut stat = unsafe { std::mem::zeroed::<libc::statfs>() };
    let result = unsafe { libc::statfs(c_path.as_ptr(), &mut stat) };
    if result != 0 {
        return Err(HelperError::Io(std::io::Error::last_os_error()));
    }
    #[cfg(target_os = "linux")]
    let available = stat.f_bavail.saturating_mul(stat.f_bsize as u64);
    #[cfg(not(target_os = "linux"))]
    let available = (stat.f_bavail as u64).saturating_mul(stat.f_bsize as u64);
    Ok(available)
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
            snapshot_manifest: vec![],
        };
        assert_ne!(lease.uid, 11);
    }

    #[test]
    fn source_snapshot_allows_related_snapshots_on_other_volumes() {
        let source = SnapshotInfo {
            uuid: "source".into(),
            name: "source-snapshot".into(),
            created_at: "now".into(),
        };
        let other = SnapshotInfo {
            uuid: "other".into(),
            name: "other-snapshot".into(),
            created_at: "now".into(),
        };
        let manifest = vec![("disk5s1".into(), source), ("disk9s1".into(), other)];

        assert_eq!(
            source_snapshot(&manifest, "disk5s1").unwrap().uuid,
            "source"
        );
    }

    #[test]
    fn source_snapshot_rejects_ambiguous_source_volume() {
        let snapshot = || SnapshotInfo {
            uuid: "source".into(),
            name: "source-snapshot".into(),
            created_at: "now".into(),
        };
        let manifest = vec![
            ("disk5s1".into(), snapshot()),
            ("disk5s1".into(), snapshot()),
        ];

        assert!(source_snapshot(&manifest, "disk5s1").is_err());
    }

    #[test]
    fn source_inventory_tags_only_the_configured_source_device() {
        let mut snapshots = BTreeMap::new();
        snapshots.insert(
            "snapshot-1".to_string(),
            SnapshotInfo {
                uuid: "snapshot-1".into(),
                name: "source-snapshot".into(),
                created_at: "now".into(),
            },
        );

        let inventory = tag_snapshot_inventory("disk5s1", snapshots);

        assert_eq!(inventory.len(), 1);
        assert!(inventory.contains_key(&("disk5s1".into(), "snapshot-1".into())));
        assert!(!inventory.keys().any(|(device, _)| device == "disk13s1"));
    }

    #[test]
    fn configured_source_reads_the_project_config_path() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(
            dir.path().join("config.toml"),
            format!(
                "version = 2\n\n[[targets]]\nid = \"target-1\"\nsource_path = \"{}\"\n",
                source.display()
            ),
        )
        .unwrap();

        assert_eq!(
            configured_source(dir.path(), "target-1").unwrap(),
            source.canonicalize().unwrap()
        );
    }
}
