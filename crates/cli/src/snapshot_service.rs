use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use plist::Value as PlistValue;
use serde_json::{Value, json};
use televybackup_snapshot_access::{
    ACCESS_AGENT_PLIST_NAME, ACCESS_BUNDLE_ID, ACCESS_BUNDLE_RELATIVE_PATH, COMPONENT_VERSION,
    MOUNT_HELPER_INSTALL_PATH, Method, PROTOCOL_VERSION, Request, Response, ResponseResult,
    StatusResult,
};
use uuid::Uuid;

use super::CliError;

pub const ACCESS_LABEL: &str = "com.ivan.televybackup.snapshot-access";
const MANIFEST_FILE: &str = "snapshot-access/service.json";
const MIGRATION_BACKUP_DIR: &str = "snapshot-access/migrations";
const MIGRATION_OWNER_TTL_SECONDS: u64 = 120;
const INSTALLED_PRODUCTION_APP_PATH: &str = "/Applications/TelevyBackup.app";
const PRODUCTION_BUNDLE_ID: &str = "com.ivan.televybackup";

fn user_service_target(domain: &str) -> String {
    format!("{domain}/{ACCESS_LABEL}")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

// This path is read only to discover and back up the v0.9.8 registration. New
// registrations never write a user LaunchAgent plist.
fn legacy_plist_path() -> PathBuf {
    std::env::var_os("TELEVYBACKUP_SNAPSHOT_ACCESS_PLIST")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            home_dir()
                .join("Library/LaunchAgents")
                .join(format!("{ACCESS_LABEL}.plist"))
        })
}

fn manifest_path(config_dir: &Path) -> PathBuf {
    config_dir.join(MANIFEST_FILE)
}

fn launchctl(args: &[&str]) -> Result<(), CliError> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|error| {
            CliError::retryable("snapshot_access.launchctl_failed", error.to_string())
        })?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(CliError::retryable(
        "snapshot_access.launchctl_failed",
        if message.is_empty() {
            format!("launchctl {} failed", args.join(" "))
        } else {
            message
        },
    ))
}

fn launchctl_not_found(error: &CliError) -> bool {
    let message = error.message.to_ascii_lowercase();
    message.contains("could not find service")
        || message.contains("service not found")
        || message.contains("unknown service")
        || message.contains("no such process")
}

fn bootout_service(service: &str) -> Result<(), CliError> {
    match launchctl(&["bootout", service]) {
        Ok(()) => Ok(()),
        Err(error) if launchctl_not_found(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

fn bootstrap_legacy_service(domain: &str, plist: &Path) -> Result<(), CliError> {
    let plist = plist.to_string_lossy();
    match launchctl(&["bootstrap", domain, plist.as_ref()]) {
        Ok(()) => Ok(()),
        Err(error) if launchctl_already_loaded(&error) => Ok(()),
        Err(error) => Err(error),
    }
}

fn launchctl_already_loaded(error: &CliError) -> bool {
    let message = error.message.to_ascii_lowercase();
    message.contains("already loaded") || message.contains("service exists")
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::new("snapshot_access.migration_failed", "path has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| CliError::new("snapshot_access.migration_failed", error.to_string()))?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| CliError::new("snapshot_access.migration_failed", error.to_string()))?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| CliError::new("snapshot_access.migration_failed", error.to_string()))?;
    fs::rename(temp, path)
        .map_err(|error| CliError::new("snapshot_access.migration_failed", error.to_string()))
}

fn main_app_path() -> Result<PathBuf, CliError> {
    let executable = std::env::current_exe().map_err(|error| {
        CliError::new(
            "snapshot_access.app_invalid",
            format!("cannot resolve the running CLI bundle: {error}"),
        )
    })?;
    executable
        .ancestors()
        .find(|path| {
            path.extension().is_some_and(|extension| extension == "app")
                && path.file_name().is_some_and(|name| {
                    name == "TelevyBackup.app" || name == "TelevyBackup Dev.app"
                })
        })
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            CliError::new(
                "snapshot_access.app_invalid",
                "Snapshot Access registration must be started by the installed TelevyBackup.app",
            )
        })
}

fn require_installed_production_app() -> Result<PathBuf, CliError> {
    let app = main_app_path()?;
    let info_plist = app.join("Contents/Info.plist");
    let bundle_id = plist::from_file::<_, PlistValue>(&info_plist)
        .ok()
        .and_then(|value| value.as_dictionary().cloned())
        .and_then(|dictionary| dictionary.get("CFBundleIdentifier").cloned())
        .and_then(PlistValue::into_string)
        .ok_or_else(|| {
            CliError::new(
                "snapshot_access.app_invalid",
                format!(
                    "cannot read production bundle identifier from {}",
                    info_plist.display()
                ),
            )
        })?;
    if bundle_id != PRODUCTION_BUNDLE_ID {
        return Err(CliError::new(
            "snapshot_access.app_invalid",
            format!(
                "Snapshot Access migration requires bundle id {PRODUCTION_BUNDLE_ID}; found {bundle_id}"
            ),
        ));
    }
    if !is_canonical_production_app(&app) {
        return Err(CliError::new(
            "snapshot_access.app_invalid",
            format!(
                "Snapshot Access migration must run from {INSTALLED_PRODUCTION_APP_PATH}; install TelevyBackup.app there before migrating"
            ),
        ));
    }
    Ok(app)
}

fn is_canonical_production_app(app: &Path) -> bool {
    app == Path::new(INSTALLED_PRODUCTION_APP_PATH)
}

fn manifest_matches_embedded_access_app(manifest: Option<&Value>, access_app: &Path) -> bool {
    manifest
        .and_then(|value| value.get("appPath"))
        .and_then(Value::as_str)
        .is_some_and(|path| Path::new(path) == access_app)
}

fn embedded_access_app_path() -> Result<PathBuf, CliError> {
    let app = main_app_path()?;
    let access_app = app.join(ACCESS_BUNDLE_RELATIVE_PATH);
    let executable = access_app.join("Contents/MacOS/televybackup-snapshot-access");
    if !executable.is_file() {
        return Err(CliError::new(
            "snapshot_access.app_invalid",
            format!(
                "embedded Snapshot Access executable not found: {}",
                executable.display()
            ),
        ));
    }
    Ok(access_app)
}

fn default_socket(data_dir: &Path) -> PathBuf {
    data_dir.join("snapshot-access/access.sock")
}

// Migration lease probing must inspect the v0.9.8 helper before the new component is registered.
// Callers that commit or use the new helper apply validate_component_status separately.
fn status_from_socket(socket: &Path) -> Result<StatusResult, CliError> {
    use std::io::{BufRead, Write};
    use std::os::unix::net::UnixStream;

    let mut stream = UnixStream::connect(socket)
        .map_err(|error| CliError::retryable("snapshot_access.unavailable", error.to_string()))?;
    let request = Request {
        version: PROTOCOL_VERSION,
        request_id: Uuid::new_v4().to_string(),
        method: Method::Status,
    };
    let mut bytes = serde_json::to_vec(&request)
        .map_err(|error| CliError::new("snapshot_access.protocol", error.to_string()))?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .map_err(|error| CliError::retryable("snapshot_access.unavailable", error.to_string()))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(12)))
        .ok();
    let mut line = String::new();
    std::io::BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| CliError::retryable("snapshot_access.unavailable", error.to_string()))?;
    let response: Response = serde_json::from_str(line.trim())
        .map_err(|error| CliError::new("snapshot_access.protocol", error.to_string()))?;
    validate_status_response(&response, &request.request_id)?;
    if !response.ok {
        return Err(CliError::new(
            "snapshot_access.rejected",
            response.message.unwrap_or_else(|| "status rejected".into()),
        ));
    }
    match response.result {
        Some(ResponseResult::Status(status)) => Ok(status),
        _ => Err(CliError::new(
            "snapshot_access.protocol",
            "status response missing",
        )),
    }
}

fn current_status_from_socket(socket: &Path) -> Result<StatusResult, CliError> {
    let status = status_from_socket(socket)?;
    validate_component_status(&status)?;
    Ok(status)
}

fn validate_component_status(status: &StatusResult) -> Result<(), CliError> {
    if status.access_app_version != COMPONENT_VERSION {
        return Err(CliError::new(
            "snapshot_access.protocol",
            format!(
                "incompatible Snapshot Access component version: {}",
                status.access_app_version
            ),
        ));
    }
    Ok(())
}

fn validate_status_response(response: &Response, request_id: &str) -> Result<(), CliError> {
    if response.version != PROTOCOL_VERSION {
        return Err(CliError::new(
            "snapshot_access.protocol",
            format!(
                "unsupported Snapshot Access response version: {}",
                response.version
            ),
        ));
    }
    if response.request_id != request_id {
        return Err(CliError::new(
            "snapshot_access.protocol",
            "Snapshot Access response request id does not match",
        ));
    }
    Ok(())
}

fn manifest_value(
    access_app: &Path,
    config_dir: &Path,
    data_dir: &Path,
    migration_state: &str,
    migration_id: &str,
    migration_owner: &str,
    migration_owner_expires_at: u64,
) -> Value {
    json!({
        "schemaVersion": 2,
        "label": ACCESS_LABEL,
        "bundleId": ACCESS_BUNDLE_ID,
        "managedBy": "smappservice",
        "plistName": ACCESS_AGENT_PLIST_NAME,
        "appPath": access_app,
        "relativeAppPath": ACCESS_BUNDLE_RELATIVE_PATH,
        "executablePath": access_app.join("Contents/MacOS/televybackup-snapshot-access"),
        "configDir": config_dir,
        "dataDir": data_dir,
        "componentVersion": COMPONENT_VERSION,
        "protocolVersion": PROTOCOL_VERSION,
        "source": "embedded-bundle",
        "migrationState": migration_state,
        "migrationId": migration_id,
        "migrationOwner": migration_owner,
        "migrationOwnerExpiresAt": migration_owner_expires_at,
    })
}

fn write_manifest(config_dir: &Path, manifest: &Value) -> Result<(), CliError> {
    let bytes = serde_json::to_vec_pretty(manifest)
        .map_err(|error| CliError::new("snapshot_access.migration_failed", error.to_string()))?;
    atomic_write(&manifest_path(config_dir), &bytes)
}

fn manifest_json(config_dir: &Path) -> Option<Value> {
    fs::read(manifest_path(config_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
}

fn manifest_data_dir(manifest: Option<&Value>, fallback: &Path) -> PathBuf {
    manifest
        .and_then(|value| value.get("dataDir"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| fallback.to_path_buf())
}

fn active_leases(manifest: Option<&Value>, data_dir: &Path) -> Result<u64, CliError> {
    status_from_socket(&default_socket(&manifest_data_dir(manifest, data_dir)))
        .map(|status| u64::from(status.active_leases))
}

fn active_legacy_leases(existing: Option<&Value>, data_dir: &Path) -> Result<u64, CliError> {
    let legacy_manifest = existing
        .and_then(|value| value.get("legacyBackup"))
        .and_then(|backup| backup.get("manifestBackupPath"))
        .and_then(Value::as_str)
        .and_then(|path| fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok());
    active_leases(legacy_manifest.as_ref().or(existing), data_dir)
}

fn acquire_migration_lock(config_dir: &Path) -> Result<std::fs::File, CliError> {
    let lock_path = config_dir.join("snapshot-access/migration.lock");
    if let Some(parent) = lock_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            CliError::new("snapshot_access.migration_failed", error.to_string())
        })?;
    }
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(lock_path)
        .map_err(|error| CliError::new("snapshot_access.migration_failed", error.to_string()))?;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if result != 0 {
        return Err(CliError::retryable(
            "snapshot_access.busy",
            "another Snapshot Access migration is in progress",
        ));
    }
    Ok(file)
}

fn migration_id() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    format!("legacy-{nanos}-{}", std::process::id())
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs())
        .unwrap_or(0)
}

fn migration_owner() -> String {
    Uuid::new_v4().to_string()
}

fn migration_owner_expiry() -> u64 {
    unix_seconds().saturating_add(MIGRATION_OWNER_TTL_SECONDS)
}

fn migration_owner_from_manifest(manifest: &Value) -> Result<&str, CliError> {
    manifest
        .get("migrationOwner")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CliError::new(
                "snapshot_access.migration_failed",
                "Snapshot Access migration has no owner token",
            )
        })
}

fn migration_owner_is_active(manifest: &Value) -> bool {
    manifest
        .get("migrationOwner")
        .and_then(Value::as_str)
        .is_some_and(|owner| !owner.is_empty())
        && manifest
            .get("migrationOwnerExpiresAt")
            .and_then(Value::as_u64)
            .is_some_and(|expires_at| expires_at > unix_seconds())
}

fn pending_response(access_app: &Path, manifest: &Value) -> Value {
    json!({
        "prepared": true,
        "managedBy": "smappservice",
        "appPath": access_app,
        "relativeAppPath": ACCESS_BUNDLE_RELATIVE_PATH,
        "migrationState": "pending",
        "migrationId": manifest.get("migrationId"),
        "migrationOwner": manifest.get("migrationOwner"),
    })
}

fn is_pending_migration(manifest: Option<&Value>) -> bool {
    manifest.is_some_and(|value| {
        value.get("managedBy").and_then(Value::as_str) == Some("smappservice")
            && value.get("relativeAppPath").and_then(Value::as_str)
                == Some(ACCESS_BUNDLE_RELATIVE_PATH)
            && value.get("migrationState").and_then(Value::as_str) == Some("pending")
    })
}

fn migration_id_from_manifest(manifest: &Value) -> Result<&str, CliError> {
    manifest
        .get("migrationId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            CliError::new(
                "snapshot_access.migration_failed",
                "Snapshot Access migration has no transaction id",
            )
        })
}

fn backup_legacy_registration(
    config_dir: &Path,
    manifest_file: &Path,
    legacy_plist: &Path,
) -> Result<Option<Value>, CliError> {
    if !legacy_plist.exists() && !manifest_file.exists() {
        return Ok(None);
    }
    let backup_dir = config_dir.join(MIGRATION_BACKUP_DIR).join(migration_id());
    fs::create_dir_all(&backup_dir)
        .map_err(|error| CliError::new("snapshot_access.migration_failed", error.to_string()))?;
    let backup_plist = if legacy_plist.is_file() {
        let destination = backup_dir.join("LaunchAgent.plist");
        fs::copy(legacy_plist, &destination).map_err(|error| {
            CliError::new("snapshot_access.migration_failed", error.to_string())
        })?;
        Some(destination)
    } else {
        None
    };
    let backup_manifest = if manifest_file.is_file() {
        let destination = backup_dir.join("service.json");
        fs::copy(manifest_file, &destination).map_err(|error| {
            CliError::new("snapshot_access.migration_failed", error.to_string())
        })?;
        Some(destination)
    } else {
        None
    };
    Ok(Some(json!({
        "originalPlistPath": legacy_plist,
        "plistBackupPath": backup_plist,
        "manifestBackupPath": backup_manifest,
        "backupDirectory": backup_dir,
    })))
}

fn restore_legacy_registration(
    config_dir: &Path,
    data_dir: &Path,
    record: &Value,
) -> Result<(), CliError> {
    let domain = format!("gui/{}", unsafe { libc::geteuid() });
    let service = user_service_target(&domain);
    bootout_service(&service)?;
    if let (Some(source), Some(destination)) = (
        record.get("plistBackupPath").and_then(Value::as_str),
        record.get("originalPlistPath").and_then(Value::as_str),
    ) {
        let destination = Path::new(destination);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                CliError::new("snapshot_access.rollback_failed", error.to_string())
            })?;
        }
        fs::copy(source, destination)
            .map_err(|error| CliError::new("snapshot_access.rollback_failed", error.to_string()))?;
        bootstrap_legacy_service(&domain, Path::new(destination))?;
    }
    if let Some(source) = record.get("manifestBackupPath").and_then(Value::as_str) {
        fs::copy(source, manifest_path(config_dir))
            .map_err(|error| CliError::new("snapshot_access.rollback_failed", error.to_string()))?;
    } else if manifest_path(config_dir).exists() {
        fs::remove_file(manifest_path(config_dir))
            .map_err(|error| CliError::new("snapshot_access.rollback_failed", error.to_string()))?;
    }
    let _ = data_dir;
    Ok(())
}

pub fn prepare_migration(
    config_dir: &Path,
    data_dir: &Path,
    json_output: bool,
) -> Result<(), CliError> {
    let _migration_lock = acquire_migration_lock(config_dir)?;
    let _app = require_installed_production_app()?;
    let access_app = embedded_access_app_path()?;
    let manifest_file = manifest_path(config_dir);
    let existing = manifest_json(config_dir);
    let legacy_plist = legacy_plist_path();
    let existing_is_current = existing
        .as_ref()
        .and_then(|value| value.get("managedBy"))
        .and_then(Value::as_str)
        == Some("smappservice")
        && existing
            .as_ref()
            .and_then(|value| value.get("relativeAppPath"))
            .and_then(Value::as_str)
            == Some(ACCESS_BUNDLE_RELATIVE_PATH)
        && manifest_matches_embedded_access_app(existing.as_ref(), &access_app)
        && existing
            .as_ref()
            .and_then(|value| value.get("migrationState"))
            .and_then(Value::as_str)
            == Some("ready");
    let pending_migration = is_pending_migration(existing.as_ref());

    if existing_is_current && !legacy_plist.exists() {
        if json_output {
            println!("{}", json!({"prepared": false, "migrationState": "ready"}));
        }
        return Ok(());
    }

    if pending_migration && !legacy_plist.exists() {
        let existing = existing.as_ref().expect("pending migration has a manifest");
        if migration_owner_is_active(existing) {
            return Err(CliError::retryable(
                "snapshot_access.busy",
                "another TelevyBackup instance owns the Snapshot Access migration",
            ));
        }
        let owner = migration_owner();
        let expiry = migration_owner_expiry();
        let mut claimed = existing.clone();
        claimed["migrationOwner"] = Value::String(owner.clone());
        claimed["migrationOwnerExpiresAt"] = json!(expiry);
        write_manifest(config_dir, &claimed)?;
        if json_output {
            println!("{}", pending_response(&access_app, &claimed));
        }
        return Ok(());
    }

    if pending_migration {
        let existing = existing.as_ref().expect("pending migration has a manifest");
        if migration_owner_is_active(existing) {
            return Err(CliError::retryable(
                "snapshot_access.busy",
                "another TelevyBackup instance owns the Snapshot Access migration",
            ));
        }
        // A crash may leave the legacy plist on disk after launchctl bootout.
        // Re-bootstrap it before probing leases so recovery remains retryable.
        let domain = format!("gui/{}", unsafe { libc::geteuid() });
        if legacy_plist.exists() {
            bootstrap_legacy_service(&domain, &legacy_plist).map_err(|error| {
                CliError::retryable(
                    "snapshot_access.busy",
                    format!(
                        "cannot restore the legacy Snapshot Access service: {}",
                        error.message
                    ),
                )
            })?;
        }
        let leases = active_legacy_leases(Some(existing), data_dir).map_err(|error| {
            CliError::retryable(
                "snapshot_access.busy",
                format!(
                    "cannot prove that Snapshot Access has no active lease: {}",
                    error.message
                ),
            )
        })?;
        if leases > 0 {
            return Err(CliError::retryable(
                "snapshot_access.busy",
                "cannot migrate Snapshot Access while a snapshot lease is active",
            ));
        }
        let service = user_service_target(&domain);
        bootout_service(&service)?;
        if legacy_plist.exists() {
            fs::remove_file(&legacy_plist).map_err(|error| {
                CliError::new("snapshot_access.migration_failed", error.to_string())
            })?;
        }
        let owner = migration_owner();
        let expiry = migration_owner_expiry();
        let mut claimed = existing.clone();
        claimed["migrationOwner"] = Value::String(owner);
        claimed["migrationOwnerExpiresAt"] = json!(expiry);
        if let Err(error) = write_manifest(config_dir, &claimed) {
            if let Some(backup) = existing.get("legacyBackup") {
                let _ = restore_legacy_registration(config_dir, data_dir, backup);
            }
            return Err(error);
        }
        if json_output {
            println!("{}", pending_response(&access_app, &claimed));
        }
        return Ok(());
    }

    let legacy_registration = legacy_plist.exists() || (existing.is_some() && !existing_is_current);
    if legacy_registration {
        let leases = active_leases(existing.as_ref(), data_dir).map_err(|error| {
            CliError::retryable(
                "snapshot_access.busy",
                format!(
                    "cannot prove that Snapshot Access has no active lease: {}",
                    error.message
                ),
            )
        })?;
        if leases > 0 {
            return Err(CliError::retryable(
                "snapshot_access.busy",
                "cannot migrate Snapshot Access while a snapshot lease is active",
            ));
        }
    }

    let backup = backup_legacy_registration(config_dir, &manifest_file, &legacy_plist)?;
    let migration_id = migration_id();
    let migration_owner = migration_owner();
    let migration_owner_expires_at = migration_owner_expiry();
    let mut manifest = manifest_value(
        &access_app,
        config_dir,
        data_dir,
        "pending",
        &migration_id,
        &migration_owner,
        migration_owner_expires_at,
    );
    if let Some(backup) = backup {
        manifest["legacyBackup"] = backup;
    }
    if let Err(error) = write_manifest(config_dir, &manifest) {
        if let Some(backup) = manifest.get("legacyBackup") {
            let _ = restore_legacy_registration(config_dir, data_dir, backup);
        }
        return Err(error);
    }

    let domain = format!("gui/{}", unsafe { libc::geteuid() });
    let service = user_service_target(&domain);
    if legacy_registration {
        if let Err(error) = bootout_service(&service) {
            if let Some(backup) = manifest.get("legacyBackup") {
                let _ = restore_legacy_registration(config_dir, data_dir, backup);
            }
            return Err(error);
        }
        if legacy_plist.exists()
            && let Err(error) = fs::remove_file(&legacy_plist)
        {
            if let Some(backup) = manifest.get("legacyBackup") {
                let _ = restore_legacy_registration(config_dir, data_dir, backup);
            }
            return Err(CliError::new(
                "snapshot_access.migration_failed",
                error.to_string(),
            ));
        }
    }

    if json_output {
        println!(
            "{}",
            json!({
                "prepared": true,
                "managedBy": "smappservice",
                "appPath": access_app,
                "relativeAppPath": ACCESS_BUNDLE_RELATIVE_PATH,
                "migrationState": "pending",
                "migrationId": migration_id,
                "migrationOwner": migration_owner,
            })
        );
    } else {
        println!(
            "Snapshot Access migration prepared for {}",
            access_app.display()
        );
    }
    Ok(())
}

pub fn commit_migration(
    config_dir: &Path,
    data_dir: &Path,
    expected_migration_id: &str,
    expected_migration_owner: &str,
    json_output: bool,
) -> Result<(), CliError> {
    let _migration_lock = acquire_migration_lock(config_dir)?;
    let _app = require_installed_production_app()?;
    let access_app = embedded_access_app_path()?;
    let mut manifest = manifest_json(config_dir).ok_or_else(|| {
        CliError::new(
            "snapshot_access.migration_failed",
            "Snapshot Access migration is not prepared",
        )
    })?;
    if manifest.get("managedBy").and_then(Value::as_str) != Some("smappservice") {
        return Err(CliError::new(
            "snapshot_access.migration_failed",
            "Snapshot Access manifest is not managed by SMAppService",
        ));
    }
    match manifest.get("migrationState").and_then(Value::as_str) {
        Some("pending") => {
            if migration_id_from_manifest(&manifest)? != expected_migration_id {
                return Err(CliError::retryable(
                    "snapshot_access.busy",
                    "Snapshot Access migration belongs to another transaction",
                ));
            }
            if migration_owner_from_manifest(&manifest)? != expected_migration_owner {
                return Err(CliError::retryable(
                    "snapshot_access.busy",
                    "Snapshot Access migration is owned by another instance",
                ));
            }
            if !migration_owner_is_active(&manifest) {
                return Err(CliError::retryable(
                    "snapshot_access.busy",
                    "Snapshot Access migration owner has expired",
                ));
            }
        }
        Some("ready") => {
            if migration_id_from_manifest(&manifest)? != expected_migration_id {
                return Err(CliError::retryable(
                    "snapshot_access.busy",
                    "Snapshot Access migration was completed by another transaction",
                ));
            }
            if json_output {
                println!("{}", json!({"committed": false, "migrationState": "ready"}));
            }
            return Ok(());
        }
        _ => {
            return Err(CliError::new(
                "snapshot_access.migration_failed",
                "Snapshot Access manifest has no pending migration",
            ));
        }
    }
    let socket = default_socket(&manifest_data_dir(Some(&manifest), data_dir));
    let status = current_status_from_socket(&socket)?;
    if status.access_app_path.as_deref() != Some(access_app.to_string_lossy().as_ref()) {
        return Err(CliError::new(
            "snapshot_access.identity_mismatch",
            "running Snapshot Access is not the embedded helper",
        ));
    }
    manifest["migrationState"] = Value::String("ready".into());
    if let Some(object) = manifest.as_object_mut() {
        object.remove("migrationOwner");
        object.remove("migrationOwnerExpiresAt");
    }
    write_manifest(config_dir, &manifest)?;
    if json_output {
        println!("{}", json!({"committed": true, "migrationState": "ready"}));
    } else {
        println!("Snapshot Access migration committed");
    }
    Ok(())
}

pub fn renew_migration(
    config_dir: &Path,
    expected_migration_id: &str,
    expected_migration_owner: &str,
    json_output: bool,
) -> Result<(), CliError> {
    let _migration_lock = acquire_migration_lock(config_dir)?;
    let _app = require_installed_production_app()?;
    let mut manifest = manifest_json(config_dir).ok_or_else(|| {
        CliError::new(
            "snapshot_access.migration_failed",
            "Snapshot Access migration is not prepared",
        )
    })?;
    if manifest.get("migrationState").and_then(Value::as_str) != Some("pending") {
        return Err(CliError::retryable(
            "snapshot_access.busy",
            "Snapshot Access migration is no longer pending",
        ));
    }
    if migration_id_from_manifest(&manifest)? != expected_migration_id {
        return Err(CliError::retryable(
            "snapshot_access.busy",
            "Snapshot Access migration belongs to another transaction",
        ));
    }
    if migration_owner_from_manifest(&manifest)? != expected_migration_owner {
        return Err(CliError::retryable(
            "snapshot_access.busy",
            "Snapshot Access migration is owned by another instance",
        ));
    }
    if !migration_owner_is_active(&manifest) {
        return Err(CliError::retryable(
            "snapshot_access.busy",
            "Snapshot Access migration owner has expired",
        ));
    }
    let expiry = migration_owner_expiry();
    manifest["migrationOwnerExpiresAt"] = json!(expiry);
    write_manifest(config_dir, &manifest)?;
    if json_output {
        println!(
            "{}",
            json!({
                "renewed": true,
                "migrationState": "pending",
                "migrationOwnerExpiresAt": expiry,
            })
        );
    }
    Ok(())
}

pub fn rollback_migration(
    config_dir: &Path,
    data_dir: &Path,
    expected_migration_id: &str,
    expected_migration_owner: &str,
    json_output: bool,
) -> Result<(), CliError> {
    let _migration_lock = acquire_migration_lock(config_dir)?;
    let _app = require_installed_production_app()?;
    let Some(manifest) = manifest_json(config_dir) else {
        return Ok(());
    };
    if manifest.get("migrationState").and_then(Value::as_str) != Some("pending") {
        return Ok(());
    }
    if migration_id_from_manifest(&manifest)? != expected_migration_id {
        return Err(CliError::retryable(
            "snapshot_access.busy",
            "Snapshot Access migration belongs to another transaction",
        ));
    }
    if migration_owner_from_manifest(&manifest)? != expected_migration_owner {
        return Err(CliError::retryable(
            "snapshot_access.busy",
            "Snapshot Access migration is owned by another instance",
        ));
    }
    if !migration_owner_is_active(&manifest) {
        return Err(CliError::retryable(
            "snapshot_access.busy",
            "Snapshot Access migration owner has expired",
        ));
    }
    let Some(backup) = manifest.get("legacyBackup") else {
        fs::remove_file(manifest_path(config_dir))
            .map_err(|error| CliError::new("snapshot_access.rollback_failed", error.to_string()))?;
        if json_output {
            println!("{}", json!({"rolledBack": true, "hadLegacy": false}));
        }
        return Ok(());
    };
    restore_legacy_registration(config_dir, data_dir, backup)?;
    if json_output {
        println!("{}", json!({"rolledBack": true}));
    } else {
        println!("Snapshot Access migration rolled back");
    }
    Ok(())
}

pub fn status(config_dir: &Path, data_dir: &Path, json_output: bool) -> Result<Value, CliError> {
    let manifest = manifest_json(config_dir);
    let socket = default_socket(&manifest_data_dir(manifest.as_ref(), data_dir));
    let (helper, status_error) = match status_from_socket(&socket) {
        Ok(status) => {
            let error = validate_component_status(&status)
                .err()
                .map(|error| error.message);
            (Some(status), error)
        }
        Err(error) => (None, Some(error.message)),
    };
    let payload = status_payload(
        manifest.as_ref(),
        helper.as_ref(),
        status_error.as_deref(),
        &socket,
    );
    if json_output {
        println!("{payload}");
    } else {
        println!(
            "snapshot access: {}",
            if payload["serviceReachable"].as_bool() == Some(true) {
                "running"
            } else {
                "unavailable"
            }
        );
    }
    Ok(payload)
}

fn status_payload(
    manifest: Option<&Value>,
    helper: Option<&StatusResult>,
    status_error: Option<&str>,
    socket: &Path,
) -> Value {
    let manifest_present = manifest.is_some();
    let registered_app_path = manifest
        .and_then(|value| value.get("appPath"))
        .and_then(Value::as_str);
    let running_app_path = helper.and_then(|value| value.access_app_path.as_deref());
    let managed_by = manifest
        .and_then(|value| value.get("managedBy"))
        .and_then(Value::as_str);
    let registration_mismatch = manifest_present
        && (managed_by != Some("smappservice")
            || matches!(
                (running_app_path, registered_app_path),
                (Some(running), Some(registered)) if running != registered
            ));
    json!({
        "installed": manifest.is_some()
            && managed_by == Some("smappservice")
            && manifest.and_then(|value| value.get("migrationState")).and_then(Value::as_str)
                == Some("ready"),
        "label": ACCESS_LABEL,
        "bundleId": ACCESS_BUNDLE_ID,
        "managedBy": managed_by,
        "appPath": running_app_path.or(registered_app_path),
        "accessAppPath": running_app_path,
        "registeredAppPath": registered_app_path,
        "relativeAppPath": manifest.and_then(|value| value.get("relativeAppPath")),
        "accessAppRegistrationMismatch": registration_mismatch,
        "migrationState": manifest.and_then(|value| value.get("migrationState")),
        "legacyRegistrationPath": manifest.and_then(|value| value.get("legacyBackup")).and_then(|value| value.get("originalPlistPath")),
        "executablePath": manifest.and_then(|value| value.get("executablePath")),
        "plistPath": manifest.and_then(|value| value.get("plistName")),
        "socketPath": socket,
        "serviceReachable": helper.is_some(),
        "accessAppError": status_error,
        "activeLeases": helper.map(|value| value.active_leases).unwrap_or(0),
        "pendingCleanup": helper.map(|value| value.pending_cleanup).unwrap_or(0),
        "accessAppVersion": helper.map(|value| value.access_app_version.clone()),
        "fdaReady": helper.map(|value| value.fda_ready).unwrap_or(false),
        "fdaCheckError": helper.and_then(|value| value.fda_check_error.clone()),
        "mountHelperPath": MOUNT_HELPER_INSTALL_PATH,
        "mountHelperReachable": helper.map(|value| value.mount_helper_reachable).unwrap_or(false),
        "mountHelperVersion": helper.and_then(|value| value.mount_helper_version.clone()),
        "mountHelperError": helper.and_then(|value| value.mount_helper_error.clone()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_registration_uses_relative_bundle_program() {
        let plist = format!(
            "<key>Label</key><string>{ACCESS_LABEL}</string><key>BundleProgram</key><string>{ACCESS_BUNDLE_RELATIVE_PATH}/Contents/MacOS/televybackup-snapshot-access</string>"
        );
        assert!(plist.contains("BundleProgram"));
        assert!(plist.contains(ACCESS_BUNDLE_RELATIVE_PATH));
        assert!(!plist.contains("ProgramArguments"));
        assert!(!plist.contains("target/macos-app"));
    }

    #[test]
    fn legacy_embedded_manifest_path_is_not_current_for_a_new_app_location() {
        let manifest = json!({
            "appPath": "/Applications/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app",
            "managedBy": "smappservice",
            "relativeAppPath": ACCESS_BUNDLE_RELATIVE_PATH,
            "migrationState": "ready",
        });
        let current = Path::new(
            "/Users/test/worktree/target/macos-app/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app",
        );

        assert!(!manifest_matches_embedded_access_app(
            Some(&manifest),
            current
        ));
    }

    #[test]
    fn current_embedded_manifest_path_is_current() {
        let current = Path::new(
            "/Users/test/worktree/target/macos-app/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app",
        );
        let manifest = json!({"appPath": current});

        assert!(manifest_matches_embedded_access_app(
            Some(&manifest),
            current
        ));
    }

    #[test]
    fn production_migration_requires_the_canonical_installed_app_path() {
        assert!(is_canonical_production_app(Path::new(
            "/Applications/TelevyBackup.app"
        )));
        assert!(!is_canonical_production_app(Path::new(
            "/Users/test/worktree/target/macos-app/TelevyBackup.app"
        )));
    }

    #[test]
    fn status_does_not_treat_legacy_registration_as_installed() {
        let manifest = json!({
            "appPath": "/Users/test/Projects/old/TelevyBackup Snapshot Access.app",
            "managedBy": "legacy-launchagent",
            "migrationState": "legacy-detected",
        });
        let payload = status_payload(Some(&manifest), None, None, Path::new("/tmp/access.sock"));
        assert_eq!(payload["installed"], false);
        assert_eq!(payload["managedBy"], "legacy-launchagent");
        assert_eq!(payload["migrationState"], "legacy-detected");
    }

    #[test]
    fn running_embedded_path_is_the_display_identity() {
        let manifest = json!({
            "appPath": "/Applications/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app",
            "managedBy": "smappservice",
        });
        let status = StatusResult {
            access_app_path: Some(
                "/Applications/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app".into(),
            ),
            ..Default::default()
        };
        let payload = status_payload(
            Some(&manifest),
            Some(&status),
            None,
            Path::new("/tmp/access.sock"),
        );
        assert_eq!(payload["accessAppRegistrationMismatch"], false);
        assert_eq!(payload["appPath"], manifest["appPath"]);
        assert_eq!(payload["accessAppPath"], manifest["appPath"]);
    }

    #[test]
    fn status_does_not_claim_a_registered_path_is_a_running_identity() {
        let manifest = json!({
            "appPath": "/Applications/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app",
            "managedBy": "smappservice",
        });
        let payload = status_payload(
            Some(&manifest),
            Some(&StatusResult::default()),
            None,
            Path::new("/tmp/access.sock"),
        );
        assert_eq!(payload["appPath"], manifest["appPath"]);
        assert!(payload["accessAppPath"].is_null());
    }

    #[test]
    fn fresh_install_without_registration_is_not_a_path_mismatch() {
        let payload = status_payload(None, None, None, Path::new("/tmp/access.sock"));
        assert_eq!(payload["installed"], false);
        assert_eq!(payload["accessAppRegistrationMismatch"], false);
    }

    #[test]
    fn active_lease_probe_fails_closed_when_helper_is_unreachable() {
        let data_dir = tempfile::tempdir().unwrap();
        let error = active_leases(None, data_dir.path()).unwrap_err();
        assert_eq!(error.code, "snapshot_access.unavailable");
    }

    #[test]
    fn status_response_requires_protocol_and_request_identity() {
        let response = Response::ok("request-1", ResponseResult::Status(StatusResult::default()));
        assert!(validate_status_response(&response, "request-1").is_ok());

        let mut wrong_version = response.clone();
        wrong_version.version += 1;
        assert_eq!(
            validate_status_response(&wrong_version, "request-1")
                .unwrap_err()
                .code,
            "snapshot_access.protocol"
        );

        let mut wrong_request = response;
        wrong_request.request_id = "request-2".into();
        assert_eq!(
            validate_status_response(&wrong_request, "request-1")
                .unwrap_err()
                .code,
            "snapshot_access.protocol"
        );
    }

    #[test]
    fn status_rejects_an_incompatible_component_version() {
        let status = StatusResult {
            access_app_version: "0.1.0".into(),
            ..Default::default()
        };
        assert_eq!(
            validate_component_status(&status).unwrap_err().code,
            "snapshot_access.protocol"
        );
    }

    #[test]
    fn migration_lock_serializes_registration_changes() {
        let config_dir = tempfile::tempdir().unwrap();
        let first = acquire_migration_lock(config_dir.path()).unwrap();
        let second = acquire_migration_lock(config_dir.path()).unwrap_err();
        assert_eq!(second.code, "snapshot_access.busy");
        drop(first);
        let third = acquire_migration_lock(config_dir.path()).unwrap();
        drop(third);
    }

    #[test]
    fn migration_owner_expiry_is_fail_closed() {
        let active = json!({
            "migrationOwner": "owner-a",
            "migrationOwnerExpiresAt": unix_seconds() + 1,
        });
        assert!(migration_owner_is_active(&active));

        let expired = json!({
            "migrationOwner": "owner-a",
            "migrationOwnerExpiresAt": unix_seconds().saturating_sub(1),
        });
        assert!(!migration_owner_is_active(&expired));
        assert!(!migration_owner_is_active(
            &json!({"migrationOwner": "owner-a"})
        ));
    }
}
