use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::json;
use sha2::{Digest, Sha256};
use televybackup_snapshot_access::{
    MOUNT_HELPER_INSTALL_PATH, MOUNT_HELPER_PROTOCOL_VERSION, MountStatusResult, StatusResult,
    mount_helper,
};

use super::CliError;

pub const MOUNT_HELPER_LABEL: &str = "com.ivan.televybackup.snapshot-mount-helper";
const MOUNT_HELPER_PLIST_PATH: &str =
    "/Library/LaunchDaemons/com.ivan.televybackup.snapshot-mount-helper.plist";
const INSTALL_READY_TIMEOUT: Duration = Duration::from_secs(20);
const INSTALL_READY_POLL_INTERVAL: Duration = Duration::from_millis(250);

fn system_service_target() -> String {
    format!("system/{MOUNT_HELPER_LABEL}")
}

fn current_executable_sibling() -> Result<PathBuf, CliError> {
    let executable = std::env::current_exe().map_err(|error| {
        CliError::new(
            "snapshot_mount_helper.executable_unavailable",
            error.to_string(),
        )
    })?;
    let parent = executable.parent().ok_or_else(|| {
        CliError::new(
            "snapshot_mount_helper.executable_unavailable",
            "CLI has no parent directory",
        )
    })?;
    let helper = parent.join("televybackup-snapshot-mount-helper");
    if !helper.is_file() {
        return Err(CliError::new(
            "snapshot_mount_helper.executable_unavailable",
            format!(
                "snapshot mount helper binary not found: {}",
                helper.display()
            ),
        ));
    }
    Ok(helper)
}

fn require_root() -> Result<(), CliError> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(CliError::new(
            "snapshot_mount_helper.admin_required",
            "snapshot mount helper installation must be run with administrator authorization",
        ));
    }
    Ok(())
}

fn launchctl(args: &[&str]) -> Result<(), CliError> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|error| {
            CliError::retryable("snapshot_mount_helper.launchctl_failed", error.to_string())
        })?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(CliError::retryable(
        "snapshot_mount_helper.launchctl_failed",
        if message.is_empty() {
            format!("launchctl {} failed", args.join(" "))
        } else {
            message
        },
    ))
}

fn plist_contents() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.ivan.televybackup.snapshot-mount-helper</string>
  <key>ProgramArguments</key><array><string>/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-mount-helper</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>StandardOutPath</key><string>/var/log/com.ivan.televybackup.snapshot-mount-helper.log</string>
  <key>StandardErrorPath</key><string>/var/log/com.ivan.televybackup.snapshot-mount-helper.log</string>
</dict></plist>
"#
}

fn stage_copy(source: &Path, destination: &Path) -> Result<PathBuf, CliError> {
    let parent = destination.parent().ok_or_else(|| {
        CliError::new(
            "snapshot_mount_helper.install_failed",
            "helper destination has no parent",
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CliError::new("snapshot_mount_helper.install_failed", error.to_string())
    })?;
    let staged = destination.with_extension(format!("tmp-{}", std::process::id()));
    if let Err(error) = fs::copy(source, &staged) {
        let _ = fs::remove_file(&staged);
        return Err(CliError::new(
            "snapshot_mount_helper.install_failed",
            error.to_string(),
        ));
    }
    if let Err(error) = fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)) {
        let _ = fs::remove_file(&staged);
        return Err(CliError::new(
            "snapshot_mount_helper.install_failed",
            error.to_string(),
        ));
    }
    if let Err(error) = set_root_owner(&staged, "snapshot mount helper") {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    Ok(staged)
}

fn stage_write(path: &Path, contents: &[u8]) -> Result<PathBuf, CliError> {
    let parent = path.parent().ok_or_else(|| {
        CliError::new(
            "snapshot_mount_helper.install_failed",
            "plist has no parent",
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        CliError::new("snapshot_mount_helper.install_failed", error.to_string())
    })?;
    let staged = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = match OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&staged)
    {
        Ok(file) => file,
        Err(error) => {
            let _ = fs::remove_file(&staged);
            return Err(CliError::new(
                "snapshot_mount_helper.install_failed",
                error.to_string(),
            ));
        }
    };
    if let Err(error) = file.write_all(contents).and_then(|_| file.sync_all()) {
        let _ = fs::remove_file(&staged);
        return Err(CliError::new(
            "snapshot_mount_helper.install_failed",
            error.to_string(),
        ));
    }
    if let Err(error) = fs::set_permissions(&staged, fs::Permissions::from_mode(0o644)) {
        let _ = fs::remove_file(&staged);
        return Err(CliError::new(
            "snapshot_mount_helper.install_failed",
            error.to_string(),
        ));
    }
    if let Err(error) = set_root_owner(&staged, "snapshot mount helper plist") {
        let _ = fs::remove_file(&staged);
        return Err(error);
    }
    Ok(staged)
}

fn discard_staged(paths: &[&Path]) {
    for path in paths {
        let _ = fs::remove_file(path);
    }
}

fn install_asset_error(error: impl std::fmt::Display) -> CliError {
    CliError::new("snapshot_mount_helper.install_failed", error.to_string())
}

fn backup_prefix(path: &Path) -> Result<String, CliError> {
    let name = path
        .file_stem()
        .or_else(|| path.file_name())
        .ok_or_else(|| install_asset_error("helper asset has no file name"))?;
    Ok(format!("{}.", name.to_string_lossy()))
}

fn backup_candidates(path: &Path) -> Result<Vec<PathBuf>, CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| install_asset_error("helper asset has no parent"))?;
    let prefix = format!("{}backup-", backup_prefix(path)?);
    let mut candidates = Vec::new();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidates),
        Err(error) => return Err(install_asset_error(error)),
    };
    for entry in entries {
        let entry = entry.map_err(install_asset_error)?;
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&prefix) {
            candidates.push(entry.path());
        }
    }
    candidates.sort();
    Ok(candidates)
}

fn quarantine_backup_candidates(path: &Path) -> Result<Vec<PathBuf>, CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| install_asset_error("helper asset has no parent"))?;
    let prefix = format!("{}uninstall-backup-", backup_prefix(path)?);
    let mut candidates = Vec::new();
    let entries = match fs::read_dir(parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(candidates),
        Err(error) => return Err(install_asset_error(error)),
    };
    for entry in entries {
        let entry = entry.map_err(install_asset_error)?;
        if entry.file_name().to_string_lossy().starts_with(&prefix) {
            candidates.push(entry.path());
        }
    }
    candidates.sort();
    Ok(candidates)
}

fn recover_interrupted_helper_transaction(
    config_dir: &Path,
    data_dir: &Path,
) -> Result<(), CliError> {
    let binary = Path::new(MOUNT_HELPER_INSTALL_PATH);
    let plist = Path::new(MOUNT_HELPER_PLIST_PATH);
    let paths = [binary, plist];
    let mut backups = Vec::new();
    let mut quarantine_backups = Vec::new();
    for path in paths {
        for backup in backup_candidates(path)? {
            if !safe_root_asset(&backup) {
                return Err(install_asset_error(format!(
                    "unsafe helper backup blocks recovery: {}",
                    backup.display()
                )));
            }
            backups.push((path, backup));
        }
        for backup in quarantine_backup_candidates(path)? {
            if !safe_root_asset(&backup) {
                return Err(install_asset_error(format!(
                    "unsafe uninstall backup blocks recovery: {}",
                    backup.display()
                )));
            }
            quarantine_backups.push((path, backup));
        }
    }
    let mut service_state = if backups.is_empty()
        && quarantine_backups.is_empty()
        && safe_root_asset(binary) == safe_root_asset(plist)
    {
        None
    } else {
        Some(helper_service_state())
    };
    if matches!(service_state, Some(HelperServiceState::Unknown)) {
        return Err(CliError::retryable(
            "snapshot_mount_helper.status_unavailable",
            "cannot recover interrupted helper assets while the launchd service state is unknown",
        ));
    }
    if service_state == Some(HelperServiceState::Loaded) && !backups.is_empty() {
        let candidate_ready = match mount_helper::status() {
            Ok(status) => {
                let candidate_ready = status.component_version.as_deref()
                    == Some(mount_helper::ROOT_MOUNT_HELPER_VERSION)
                    && status.build_commit.as_deref()
                        == Some(env!("TELEVYBACKUP_BUILD_COMMIT"));
                if !candidate_ready
                    && !matches!(
                        classify_busy_status("recover interrupted helper transaction", &status),
                        Ok(BusyCheck::Clear)
                    )
                {
                    return Err(CliError::retryable(
                        "snapshot_mount_helper.busy",
                        "cannot recover an interrupted helper while mount records remain active or unclassified",
                    ));
                }
                candidate_ready
            }
            Err(_) => false,
        };
        if !candidate_ready {
            let access_status = snapshot_access_status_for_maintenance(
                config_dir,
                data_dir,
                "recovering an interrupted helper transaction",
            )?;
            ensure_snapshot_access_idle(&access_status)?;
            let complete_backup_set = paths.iter().all(|path| {
                backups
                    .iter()
                    .filter(|(candidate, _)| candidate.as_os_str() == path.as_os_str())
                    .count()
                    == 1
            });
            if !complete_backup_set {
                return Err(CliError::retryable(
                    "snapshot_mount_helper.status_unavailable",
                    "cannot recover an unready helper with an incomplete or ambiguous asset backup",
                ));
            }
            bootout_for_mutation(
                &system_service_target(),
                "recover interrupted helper transaction",
                "snapshot_mount_helper.install_rollback_failed",
            )?;
            service_state = Some(HelperServiceState::Absent);
        }
    }
    for (path, backup) in backups {
        match fs::symlink_metadata(path) {
            Ok(_) if !safe_root_asset(path) => {
                return Err(install_asset_error(format!(
                    "unsafe helper asset blocks recovery: {}",
                    path.display()
                )));
            }
            Ok(_) => {
                if service_state == Some(HelperServiceState::Loaded) {
                    fs::remove_file(&backup).map_err(install_asset_error)?;
                } else if service_state == Some(HelperServiceState::Absent) {
                    let candidates = backup_candidates(path)?;
                    if candidates.len() != 1 {
                        return Err(install_asset_error(format!(
                            "ambiguous helper backup recovery for {}",
                            path.display()
                        )));
                    }
                    fs::rename(candidates[0].as_path(), path).map_err(install_asset_error)?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if service_state != Some(HelperServiceState::Absent) {
                    return Err(CliError::retryable(
                        "snapshot_mount_helper.status_unavailable",
                        "cannot restore an interrupted helper asset while the service is loaded",
                    ));
                }
                let candidates = backup_candidates(path)?;
                if candidates.len() != 1 {
                    return Err(install_asset_error(format!(
                        "ambiguous helper backup recovery for {}",
                        path.display()
                    )));
                }
                fs::rename(candidates[0].as_path(), path).map_err(install_asset_error)?;
            }
            Err(error) => return Err(install_asset_error(error)),
        }
    }
    for (path, backup) in quarantine_backups {
        if service_state == Some(HelperServiceState::Loaded) && !safe_root_asset(path) {
            return Err(CliError::retryable(
                "snapshot_mount_helper.status_unavailable",
                "cannot discard an interrupted uninstall while the service is loaded without its canonical assets",
            ));
        }
        fs::remove_file(backup).map_err(install_asset_error)?;
    }
    if service_state == Some(HelperServiceState::Absent) {
        let binary_exists = fs::symlink_metadata(binary).is_ok();
        let plist_exists = fs::symlink_metadata(plist).is_ok();
        if binary_exists != plist_exists {
            let orphan = if binary_exists { binary } else { plist };
            if !safe_root_asset(orphan) {
                return Err(install_asset_error(format!(
                    "unsafe partial helper asset blocks recovery: {}",
                    orphan.display()
                )));
            }
            fs::remove_file(orphan).map_err(install_asset_error)?;
        }
    }
    Ok(())
}

fn backup_asset_with_suffix(path: &Path, suffix: &str) -> Result<Option<PathBuf>, CliError> {
    match fs::symlink_metadata(path) {
        Ok(_) if !safe_root_asset(path) => {
            return Err(install_asset_error(format!(
                "unsafe existing helper asset: {}",
                path.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(install_asset_error(error)),
    }
    let backup = path.with_extension(format!("{suffix}-{}", std::process::id()));
    match fs::symlink_metadata(&backup) {
        Ok(_) => {
            return Err(install_asset_error(format!(
                "stale helper backup exists: {}",
                backup.display()
            )));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(install_asset_error(error)),
    }
    fs::rename(path, &backup).map_err(install_asset_error)?;
    Ok(Some(backup))
}

fn backup_asset(path: &Path) -> Result<Option<PathBuf>, CliError> {
    backup_asset_with_suffix(path, "backup")
}

fn quarantine_asset(path: &Path) -> Result<Option<PathBuf>, CliError> {
    backup_asset_with_suffix(path, "uninstall-backup")
}

fn restore_asset(path: &Path, backup: Option<PathBuf>) -> Result<(), CliError> {
    if let Some(backup) = backup {
        if !safe_root_asset(&backup) {
            return Err(install_asset_error(format!(
                "unsafe helper backup cannot be restored: {}",
                backup.display()
            )));
        }
        match fs::symlink_metadata(path) {
            Ok(_) if !safe_root_asset(path) => {
                return Err(install_asset_error(format!(
                    "unsafe helper asset blocks restore: {}",
                    path.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(install_asset_error(error)),
            Ok(_) => {}
        }
        fs::rename(backup, path).map_err(install_asset_error)?;
    }
    Ok(())
}

fn bootstrap_restored_helper() -> Result<(), CliError> {
    if Path::new(MOUNT_HELPER_PLIST_PATH).is_file() {
        launchctl(&["bootstrap", "system", MOUNT_HELPER_PLIST_PATH])?;
        wait_for_helper_status().map(|_| ())?;
    }
    Ok(())
}

fn restore_committed_assets(
    binary_backup: Option<PathBuf>,
    plist_backup: Option<PathBuf>,
) -> Result<(), CliError> {
    let mut failures = Vec::new();
    let binary = Path::new(MOUNT_HELPER_INSTALL_PATH);
    let plist = Path::new(MOUNT_HELPER_PLIST_PATH);
    for (label, path, backup) in [
        ("binary", binary, binary_backup),
        ("plist", plist, plist_backup),
    ] {
        match backup {
            Some(backup) => {
                if let Err(error) = restore_asset(path, Some(backup)) {
                    failures.push(format!("{label} restore failed: {}", error.message));
                }
            }
            None => match fs::symlink_metadata(path) {
                Ok(_) if !safe_root_asset(path) => failures.push(format!(
                    "unsafe candidate {label} blocks rollback: {}",
                    path.display()
                )),
                Ok(_) => {
                    if let Err(error) = fs::remove_file(path) {
                        failures.push(format!("candidate {label} removal failed: {error}"));
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    failures.push(format!("candidate {label} inspection failed: {error}"))
                }
            },
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(CliError::retryable(
            "snapshot_mount_helper.install_rollback_failed",
            failures.join("; "),
        ))
    }
}

fn rollback_failure(original: &CliError, rollback: CliError) -> CliError {
    CliError::retryable(
        "snapshot_mount_helper.install_rollback_failed",
        format!(
            "{}; rollback failed: {}",
            original.message, rollback.message
        ),
    )
}

fn commit_staged_assets(
    staged_binary: &Path,
    staged_plist: &Path,
) -> Result<(Option<PathBuf>, Option<PathBuf>), CliError> {
    let binary = Path::new(MOUNT_HELPER_INSTALL_PATH);
    let plist = Path::new(MOUNT_HELPER_PLIST_PATH);
    let old_binary = backup_asset(binary)?;
    let old_plist = match backup_asset(plist) {
        Ok(backup) => backup,
        Err(error) => {
            return match restore_asset(binary, old_binary) {
                Ok(()) => Err(error),
                Err(rollback) => Err(rollback_failure(&error, rollback)),
            };
        }
    };
    if let Err(error) = fs::rename(staged_binary, binary) {
        let install_error = install_asset_error(error);
        return match restore_committed_assets(old_binary, old_plist) {
            Ok(()) => Err(install_error),
            Err(rollback) => Err(rollback_failure(&install_error, rollback)),
        };
    }
    if let Err(error) = fs::rename(staged_plist, plist) {
        let install_error = install_asset_error(error);
        return match restore_committed_assets(old_binary, old_plist) {
            Ok(()) => Err(install_error),
            Err(rollback) => Err(rollback_failure(&install_error, rollback)),
        };
    }
    Ok((old_binary, old_plist))
}

fn rollback_installed_assets(
    old_binary: Option<PathBuf>,
    old_plist: Option<PathBuf>,
) -> Result<(), CliError> {
    let mutation_lock = mount_helper::acquire_mutation_lock().map_err(|error| {
        CliError::retryable(
            "snapshot_mount_helper.install_rollback_failed",
            format!("cannot reacquire the snapshot mount mutation lock: {error}"),
        )
    })?;
    match mount_helper::status() {
        Ok(status) => {
            if status.active_mounts > 0 || status.pending_cleanup.is_some_and(|count| count > 0) {
                return Err(CliError::retryable(
                    "snapshot_mount_helper.install_rollback_failed",
                    format!(
                        "cannot rollback while the helper reports {} active mount record(s) and {} pending cleanup record(s)",
                        status.active_mounts,
                        status.pending_cleanup.unwrap_or(0)
                    ),
                ));
            }
        }
        Err(error) => {
            let service_state = helper_service_state();
            if service_state == HelperServiceState::Unknown {
                return Err(CliError::retryable(
                    "snapshot_mount_helper.install_rollback_failed",
                    format!(
                        "cannot verify the helper is idle before rollback: {error}; service state is {service_state:?}"
                    ),
                ));
            }
            // A failed bootstrap can leave launchd with an absent or loaded-but-unreachable
            // candidate. The mutation lock prevents new journal-changing requests while the
            // service is booted out, and the restored helper will recover any preserved journal
            // rows on startup. An unknown launchd state remains fail-closed above.
        }
    }
    let service = system_service_target();
    if let Err(error) = bootout_for_mutation(
        &service,
        "install rollback",
        "snapshot_mount_helper.install_failed",
    ) {
        return Err(CliError::retryable(
            "snapshot_mount_helper.install_rollback_failed",
            format!(
                "cannot stop the candidate helper before rollback: {}",
                error.message
            ),
        ));
    }
    let restore_result = restore_committed_assets(old_binary, old_plist);
    drop(mutation_lock);
    restore_result?;
    bootstrap_restored_helper()
}

fn quarantine_uninstall_assets() -> Result<(Option<PathBuf>, Option<PathBuf>), CliError> {
    let plist = Path::new(MOUNT_HELPER_PLIST_PATH);
    let binary = Path::new(MOUNT_HELPER_INSTALL_PATH);
    let old_plist = quarantine_asset(plist)?;
    let old_binary = match quarantine_asset(binary) {
        Ok(backup) => backup,
        Err(error) => {
            return match restore_asset(plist, old_plist) {
                Ok(()) => Err(error),
                Err(rollback) => Err(rollback_failure(&error, rollback)),
            };
        }
    };
    Ok((old_binary, old_plist))
}

fn set_root_owner(path: &Path, description: &str) -> Result<(), CliError> {
    let encoded = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        CliError::new(
            "snapshot_mount_helper.install_failed",
            format!("{description} path contains a NUL byte"),
        )
    })?;
    let result = unsafe { libc::chown(encoded.as_ptr(), 0, 0) };
    if result != 0 {
        return Err(CliError::new(
            "snapshot_mount_helper.install_failed",
            format!(
                "cannot set {description} ownership to root:wheel: {}",
                std::io::Error::last_os_error()
            ),
        ));
    }
    Ok(())
}

fn safe_root_asset(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.is_file()
            && metadata.uid() == 0
            && metadata.gid() == 0
            && metadata.mode() & 0o022 == 0
    })
}

fn helper_assets_are_installed() -> bool {
    safe_root_asset(Path::new(MOUNT_HELPER_INSTALL_PATH))
        || safe_root_asset(Path::new(MOUNT_HELPER_PLIST_PATH))
}

fn is_uninstall_operation(operation: &str) -> bool {
    operation == "uninstall the snapshot mount helper"
}

fn no_assets_busy_check(operation: &str) -> Result<BusyCheck, CliError> {
    if is_uninstall_operation(operation) {
        Err(CliError::retryable(
            "snapshot_mount_helper.status_unavailable",
            "cannot uninstall the snapshot mount helper while its installed assets and status are unavailable; refusing to proceed",
        ))
    } else {
        Ok(BusyCheck::Clear)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HelperServiceState {
    Loaded,
    Absent,
    Unknown,
}

fn classify_helper_service_state(success: bool, output: &[u8]) -> HelperServiceState {
    if success {
        return HelperServiceState::Loaded;
    }
    let output = String::from_utf8_lossy(output);
    if output.contains("Could not find service") || output.contains("No such process") {
        HelperServiceState::Absent
    } else {
        HelperServiceState::Unknown
    }
}

fn helper_service_state() -> HelperServiceState {
    let output = match Command::new("/bin/launchctl")
        .args(["print", &system_service_target()])
        .output()
    {
        Ok(output) => output,
        Err(_) => return HelperServiceState::Unknown,
    };
    let mut diagnostic = output.stdout;
    diagnostic.extend_from_slice(&output.stderr);
    classify_helper_service_state(output.status.success(), &diagnostic)
}

fn wait_for_helper_status() -> Result<MountStatusResult, CliError> {
    let started = Instant::now();
    let mut last_error = "no ready status received".to_string();
    while started.elapsed() < INSTALL_READY_TIMEOUT {
        match mount_helper::status() {
            Ok(status) => return Ok(status),
            Err(error) => last_error = error.to_string(),
        }
        let remaining = INSTALL_READY_TIMEOUT.saturating_sub(started.elapsed());
        if !remaining.is_zero() {
            std::thread::sleep(INSTALL_READY_POLL_INTERVAL.min(remaining));
        }
    }
    Err(CliError::retryable(
        "snapshot_mount_helper.status_unavailable",
        format!(
            "cannot read the installed snapshot mount helper state after bootstrap: {last_error}"
        ),
    ))
}

fn busy_error(operation: &str, status: &MountStatusResult) -> Option<CliError> {
    if status.active_mounts == 0 {
        let current_component =
            status.component_version.as_deref() == Some(mount_helper::ROOT_MOUNT_HELPER_VERSION);
        let classification_complete = status.pending_cleanup.is_some()
            && status.live_mounts.is_some()
            && status.recoverable_stale_mounts.is_some()
            && status.unknown_blocking_mounts.is_some()
            && status.inventory_drift_mounts.is_some();
        let has_blocking_detail = status.pending_cleanup.is_some_and(|count| count > 0)
            || status.live_mounts.is_some_and(|count| count > 0)
            || status
                .recoverable_stale_mounts
                .is_some_and(|count| count > 0)
            || status
                .unknown_blocking_mounts
                .is_some_and(|count| count > 0)
            || status.inventory_drift_mounts.is_some_and(|count| count > 0)
            || !status.pending_cleanup_leases.is_empty()
            || !status.recovery_leases.is_empty()
            || !status.blocked_volume_uuids.is_empty();
        if (current_component && !classification_complete) || has_blocking_detail {
            return Some(CliError::retryable(
                "snapshot_mount_helper.status_incomplete",
                format!(
                    "cannot {operation}: the installed snapshot mount helper reported an incomplete or contradictory zero-row status"
                ),
            ));
        }
        // The legacy aggregate includes every blocking journal row, including cleanup-pending
        // rows. A legacy helper that reports zero therefore has no row to protect, even though it
        // cannot provide the additive classification fields.
        return None;
    }
    let pending_cleanup = match status.pending_cleanup {
        Some(value) => value.min(status.active_mounts),
        None => {
            return Some(CliError::retryable(
                "snapshot_mount_helper.status_incomplete",
                format!(
                    "cannot {operation}: the installed snapshot mount helper did not report a complete cleanup status"
                ),
            ));
        }
    };
    if status.inventory_drift_mounts == Some(status.active_mounts)
        && status.live_mounts == Some(0)
        && status.recoverable_stale_mounts == Some(0)
        && status.unknown_blocking_mounts == Some(0)
    {
        let count = status.inventory_drift_mounts.unwrap_or(0);
        return Some(CliError::new(
            "snapshot_mount_helper.inventory_drift",
            format!(
                "cannot {operation}: {count} released snapshot inventory record(s) require explicit reconciliation"
            ),
        ));
    }
    let in_flight = status.active_mounts.saturating_sub(pending_cleanup);
    if pending_cleanup > 0 && in_flight == 0 {
        return Some(CliError::new(
            "snapshot_mount_helper.cleanup_pending",
            format!(
                "cannot {operation}: {pending_cleanup} snapshot cleanup record(s) are pending; no active snapshot mount is running, but the helper remains fail-closed until the recorded inventory is reconciled",
            ),
        ));
    }
    let detail = match (in_flight, pending_cleanup) {
        (in_flight, pending_cleanup) if pending_cleanup > 0 => format!(
            "{in_flight} active snapshot mount(s) and {pending_cleanup} pending cleanup record(s)"
        ),
        (in_flight, _) => format!("{in_flight} active snapshot mount(s)"),
    };
    Some(CliError::new(
        "snapshot_mount_helper.busy",
        format!("cannot {operation} while {detail}"),
    ))
}

#[cfg(any(not(unix), test))]
fn pending_cleanup_leases(status: &MountStatusResult) -> Result<Vec<String>, CliError> {
    let pending_cleanup = status.pending_cleanup.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot retry snapshot cleanup: the installed snapshot mount helper did not report a complete cleanup status",
        )
    })?;
    if status.active_mounts != pending_cleanup
        || status.inventory_drift_mounts != Some(0)
        || pending_cleanup as usize != status.pending_cleanup_leases.len()
        || status
            .pending_cleanup_leases
            .iter()
            .any(|lease| lease.state != "cleanup_pending")
    {
        return Err(CliError::retryable(
            "snapshot_mount_helper.status_inconsistent",
            "cannot retry snapshot cleanup: the helper reported an inconsistent cleanup inventory",
        ));
    }
    Ok(status
        .pending_cleanup_leases
        .iter()
        .map(|lease| lease.lease_id.clone())
        .collect())
}

#[cfg(any(not(unix), test))]
fn cleanup_retry_identity(status: &MountStatusResult) -> Result<(), CliError> {
    let wire_compatible = status.helper_version == mount_helper::MOUNT_HELPER_WIRE_VERSION
        || status.helper_version == mount_helper::PREVIOUS_ROOT_MOUNT_HELPER_VERSION;
    let component_compatible = match status.component_version.as_deref() {
        Some(version) if version == mount_helper::ROOT_MOUNT_HELPER_VERSION => {
            status.build_commit.as_deref() == Some(env!("TELEVYBACKUP_BUILD_COMMIT"))
        }
        Some(version) if version == mount_helper::PREVIOUS_ROOT_MOUNT_HELPER_VERSION => {
            status.build_commit.as_deref().is_some_and(|commit| {
                commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        }
        _ => false,
    };
    if !wire_compatible || !component_compatible {
        return Err(CliError::retryable(
            "snapshot_mount_helper.update_required",
            "cannot retry snapshot cleanup until a cleanup-capable helper is installed",
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BusyCheck {
    Clear,
    LegacyUpgrade,
    PendingCleanupUpgrade,
    RecoveryRequired,
}

fn classify_busy_status(
    operation: &str,
    status: &MountStatusResult,
) -> Result<BusyCheck, CliError> {
    // Helpers predating the additive status fields cannot distinguish a live mount from a stale
    // journal row. A build identity may be present on an intermediate helper release, so the
    // known wire version and the complete absence of classification fields are the compatibility
    // signal. These helpers are eligible only for the guarded replacement transaction below,
    // never for direct cleanup or a serviceable lease.
    let known_legacy_wire = matches!(
        status.helper_version.as_str(),
        mount_helper::PREVIOUS_ROOT_MOUNT_HELPER_VERSION
            | mount_helper::LEGACY_ROOT_MOUNT_HELPER_VERSION
            | mount_helper::HISTORICAL_ROOT_MOUNT_HELPER_VERSION
    );
    let classification_missing = status.component_version.is_none()
        && status.pending_cleanup.is_none()
        && status.live_mounts.is_none()
        && status.recoverable_stale_mounts.is_none()
        && status.unknown_blocking_mounts.is_none()
        && status.inventory_drift_mounts.is_none();
    let stale_current_partition_is_complete = match (
        status.pending_cleanup,
        status.live_mounts,
        status.recoverable_stale_mounts,
        status.unknown_blocking_mounts,
    ) {
        (Some(pending), Some(live), Some(recoverable), Some(unknown)) => {
            pending
                .checked_add(live)
                .and_then(|total| total.checked_add(recoverable))
                .and_then(|total| total.checked_add(unknown))
                == Some(status.active_mounts)
        }
        _ => false,
    };
    let stale_current_partial_classification = status.helper_version
        == mount_helper::MOUNT_HELPER_WIRE_VERSION
        && status.component_version.as_deref() == Some(mount_helper::ROOT_MOUNT_HELPER_VERSION)
        && status.build_commit.as_deref().is_some_and(|commit| {
            commit != env!("TELEVYBACKUP_BUILD_COMMIT")
                && commit.len() == 40
                && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        && status.pending_cleanup.is_some()
        && status.live_mounts.is_some()
        && status.recoverable_stale_mounts.is_some()
        && status.unknown_blocking_mounts.is_some()
        && status.inventory_drift_mounts.is_none()
        && status.live_mounts == Some(0)
        && stale_current_partition_is_complete;
    if stale_current_partial_classification {
        // The previous helper implementation used the current component version but did not
        // report inventory-drift rows. Replace that known stale build only after Snapshot Access
        // proves idle; the candidate must classify the preserved journal before it can serve a
        // lease. This avoids a permanent update deadlock without treating the unknown row as
        // safe or clearing it during the upgrade.
        return Ok(BusyCheck::LegacyUpgrade);
    }
    if status.component_version.as_deref() == Some(mount_helper::ROOT_MOUNT_HELPER_VERSION)
        && status.inventory_drift_mounts.is_none()
        && status.active_mounts > 0
    {
        return Err(CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            format!(
                "cannot {operation}: the installed snapshot mount helper did not report inventory-drift classification"
            ),
        ));
    }
    let legacy_pending_only = status.helper_version == mount_helper::MOUNT_HELPER_WIRE_VERSION
        && status.component_version.as_deref()
            == Some(mount_helper::PREVIOUS_ROOT_MOUNT_HELPER_VERSION)
        && status.build_commit.as_deref().is_some_and(|commit| {
            commit.len() == 40 && commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        })
        && status.active_mounts > 0
        && status.pending_cleanup == Some(status.active_mounts)
        && status.live_mounts == Some(0)
        && status.recoverable_stale_mounts == Some(0)
        && status.unknown_blocking_mounts == Some(0)
        && status
            .inventory_drift_mounts
            .map(|count| count == 0)
            .unwrap_or(true)
        && status.pending_cleanup_leases.is_empty();
    if legacy_pending_only {
        return Ok(BusyCheck::LegacyUpgrade);
    }
    let current_pending_only = (status.helper_version == mount_helper::MOUNT_HELPER_WIRE_VERSION
        || status.helper_version == mount_helper::ROOT_MOUNT_HELPER_VERSION)
        && status.component_version.as_deref() == Some(mount_helper::ROOT_MOUNT_HELPER_VERSION)
        && status.active_mounts > 0
        && status.pending_cleanup == Some(status.active_mounts)
        && status.live_mounts == Some(0)
        && status.recoverable_stale_mounts == Some(0)
        && status.unknown_blocking_mounts == Some(0)
        && status.pending_cleanup_leases.len() == status.active_mounts as usize
        && status
            .pending_cleanup_leases
            .iter()
            .all(|lease| lease.state == "cleanup_pending")
        && status.inventory_drift_mounts == Some(0)
        && status.build_commit.as_deref() != Some(env!("TELEVYBACKUP_BUILD_COMMIT"));
    if current_pending_only {
        // A current component with a different build can be replaced only when the helper has
        // proved that every blocking row is an exact cleanup candidate and no live/unknown row
        // remains. The new helper then owns the explicit retry operation.
        return Ok(BusyCheck::PendingCleanupUpgrade);
    }
    if status.active_mounts > 0 && known_legacy_wire && classification_missing {
        return Ok(BusyCheck::LegacyUpgrade);
    }
    busy_error(operation, status).map_or(Ok(BusyCheck::Clear), Err)
}

fn ensure_uninstall_safe(check: BusyCheck) -> Result<(), CliError> {
    match check {
        BusyCheck::Clear => Ok(()),
        BusyCheck::LegacyUpgrade | BusyCheck::PendingCleanupUpgrade => Err(CliError::retryable(
            "snapshot_mount_helper.busy",
            "cannot uninstall the snapshot mount helper while snapshot mount records remain; update or reconcile the helper first",
        )),
        BusyCheck::RecoveryRequired => Err(CliError::retryable(
            "snapshot_mount_helper.status_unavailable",
            "cannot uninstall the snapshot mount helper while its status is unavailable",
        )),
    }
}

fn launchctl_service_absent(error: &CliError) -> bool {
    error.message.contains("Could not find service") || error.message.contains("No such process")
}

fn bootout_failure(error: CliError, operation: &str, error_code: &'static str) -> CliError {
    CliError::retryable(
        error_code,
        format!(
            "cannot stop the snapshot mount helper before {operation}: {}",
            error.message
        ),
    )
}

fn bootout_for_mutation(
    service: &str,
    operation: &str,
    error_code: &'static str,
) -> Result<(), CliError> {
    match launchctl(&["bootout", service]) {
        Ok(()) => Ok(()),
        Err(error) if launchctl_service_absent(&error) => Ok(()),
        Err(error) => Err(bootout_failure(error, operation, error_code)),
    }
}

fn ensure_snapshot_access_idle(status: &StatusResult) -> Result<(), CliError> {
    if status.active_leases > 0 {
        return Err(CliError::retryable(
            "snapshot_mount_helper.active",
            format!(
                "cannot replace a legacy snapshot mount helper while Snapshot Access has {} active lease(s) and {} pending cleanup record(s)",
                status.active_leases, status.pending_cleanup
            ),
        ));
    }
    if status.pending_cleanup > 0 {
        return Err(CliError::retryable(
            "snapshot_mount_helper.access_cleanup_pending",
            format!(
                "cannot continue while Snapshot Access is retrying {} pending cleanup record(s)",
                status.pending_cleanup
            ),
        ));
    }
    Ok(())
}

fn snapshot_access_status_for_maintenance(
    config_dir: &Path,
    data_dir: &Path,
    operation: &str,
) -> Result<StatusResult, CliError> {
    super::snapshot_service::status_for_maintenance(config_dir, data_dir).map_err(|error| {
        CliError::retryable(
            "snapshot_mount_helper.access_unavailable",
            format!(
                "cannot verify Snapshot Access before {operation}: {}",
                error.message
            ),
        )
    })
}

fn wait_for_snapshot_access_idle(
    config_dir: &Path,
    data_dir: &Path,
    operation: &str,
) -> Result<StatusResult, CliError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = snapshot_access_status_for_maintenance(config_dir, data_dir, operation)?;
        if status.active_leases > 0 || Instant::now() >= deadline {
            ensure_snapshot_access_idle(&status)?;
        }
        if status.pending_cleanup == 0 {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn wait_for_legacy_upgrade(config_dir: &Path, data_dir: &Path) -> Result<(), CliError> {
    // Snapshot Access cleanup retries acquire the same cross-process mutation lock as an
    // installer. Do not hold that lock while waiting, or the cleanup can never make progress.
    wait_for_snapshot_access_idle(
        config_dir,
        data_dir,
        "replacing the legacy snapshot mount helper",
    )?;
    Ok(())
}

fn check_if_busy(operation: &str) -> Result<BusyCheck, CliError> {
    match mount_helper::status() {
        Ok(status) => classify_busy_status(operation, &status),
        Err(_error) if !helper_assets_are_installed() => no_assets_busy_check(operation),
        Err(error) => match helper_service_state() {
            HelperServiceState::Absent if safe_root_asset(Path::new(MOUNT_HELPER_PLIST_PATH)) => {
                // An explicitly absent helper may have left journal rows behind. The caller must
                // release the mutation lock before bootstrapping so the helper can recover.
                Ok(BusyCheck::RecoveryRequired)
            }
            HelperServiceState::Absent => Err(CliError::retryable(
                "snapshot_mount_helper.status_unavailable",
                format!(
                    "cannot {operation}: the installed snapshot mount helper service is absent and its root-owned launchd plist is unavailable ({error})"
                ),
            )),
            HelperServiceState::Loaded | HelperServiceState::Unknown => Err(CliError::retryable(
                "snapshot_mount_helper.status_unavailable",
                format!(
                    "cannot {operation}: the installed snapshot mount helper state could not be verified; refusing to proceed ({error})"
                ),
            )),
        },
    }
}

fn recover_helper_before_mutation() -> Result<(), CliError> {
    launchctl(&["bootstrap", "system", MOUNT_HELPER_PLIST_PATH])?;
    wait_for_helper_status().map(|_| ())
}

fn wait_for_installed_helper(
    expected_commit: &str,
    mut probe: impl FnMut() -> Result<MountStatusResult, mount_helper::MountHelperError>,
    mut pause: impl FnMut(Duration),
    mut elapsed: impl FnMut() -> Duration,
) -> Result<MountStatusResult, CliError> {
    let mut last_error = "no ready status received".to_string();
    // Connection refusal returns immediately, before the status read timeout.
    // Bound startup by elapsed time instead of exhausting four fast probes.
    // An in-flight status request retains its existing five-second I/O timeout.
    while elapsed() < INSTALL_READY_TIMEOUT {
        match probe() {
            Ok(status)
                if status.component_version.as_deref()
                    == Some(mount_helper::ROOT_MOUNT_HELPER_VERSION)
                    && status.build_commit.as_deref() == Some(expected_commit)
                    && expected_commit.len() == 40
                    && expected_commit.bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                if elapsed() < INSTALL_READY_TIMEOUT {
                    return Ok(status);
                }
                last_error = "candidate status arrived after the startup deadline".to_string();
            }
            Ok(status) => {
                last_error = format!(
                    "expected component {} and candidate build; received component {} with {} build identity",
                    mount_helper::ROOT_MOUNT_HELPER_VERSION,
                    status
                        .component_version
                        .as_deref()
                        .unwrap_or(&status.helper_version),
                    if status.build_commit.is_some() {
                        "different"
                    } else {
                        "missing"
                    }
                );
            }
            Err(error) => last_error = error.to_string(),
        }
        let remaining = INSTALL_READY_TIMEOUT.saturating_sub(elapsed());
        if !remaining.is_zero() {
            pause(INSTALL_READY_POLL_INTERVAL.min(remaining));
        }
    }
    Err(CliError::retryable(
        "snapshot_mount_helper.readiness_failed",
        format!(
            "mount helper was bootstrapped but did not become ready within the {}s startup window (expected protocol {MOUNT_HELPER_PROTOCOL_VERSION}): {last_error}",
            INSTALL_READY_TIMEOUT.as_secs()
        ),
    ))
}

fn helper_sha256(path: &Path) -> Result<String, CliError> {
    let mut file = fs::File::open(path).map_err(|error| {
        CliError::new("snapshot_mount_helper.identity_failed", error.to_string())
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer).map_err(|error| {
            CliError::new("snapshot_mount_helper.identity_failed", error.to_string())
        })?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

pub fn install(config_dir: &Path, data_dir: &Path, json_output: bool) -> Result<(), CliError> {
    require_root()?;
    let _install_lock = mount_helper::acquire_install_lock().map_err(|error| {
        CliError::retryable(
            "snapshot_mount_helper.install_lock_unavailable",
            format!("cannot install or update the snapshot mount helper safely: {error}"),
        )
    })?;
    let mut mutation_lock = mount_helper::acquire_mutation_lock().map_err(|error| {
        CliError::retryable(
            "snapshot_mount_helper.lock_unavailable",
            format!("cannot install or update the snapshot mount helper safely: {error}"),
        )
    })?;
    recover_interrupted_helper_transaction(config_dir, data_dir)?;
    let busy = check_if_busy("install or update the snapshot mount helper")?;
    let mut replace_legacy_helper = false;
    match busy {
        BusyCheck::Clear => {}
        BusyCheck::LegacyUpgrade | BusyCheck::PendingCleanupUpgrade => {
            replace_legacy_helper = true;
        }
        BusyCheck::RecoveryRequired => {
            drop(mutation_lock);
            recover_helper_before_mutation()?;
            mutation_lock = mount_helper::acquire_mutation_lock().map_err(|error| {
                CliError::retryable(
                    "snapshot_mount_helper.lock_unavailable",
                    format!("cannot resume helper installation safely after recovery: {error}"),
                )
            })?;
            match check_if_busy("install or update the snapshot mount helper")? {
                BusyCheck::Clear => {}
                BusyCheck::LegacyUpgrade | BusyCheck::PendingCleanupUpgrade => {
                    replace_legacy_helper = true;
                }
                BusyCheck::RecoveryRequired => {
                    return Err(CliError::retryable(
                        "snapshot_mount_helper.status_unavailable",
                        "snapshot mount helper remained absent after recovery bootstrap",
                    ));
                }
            }
        }
    }
    recover_interrupted_helper_transaction(config_dir, data_dir)?;
    let (staged_binary, staged_plist, candidate_hash) = {
        let source = current_executable_sibling()?;
        let candidate_hash = helper_sha256(&source)?;
        let staged_binary = stage_copy(&source, Path::new(MOUNT_HELPER_INSTALL_PATH))?;
        let staged_plist = match stage_write(
            Path::new(MOUNT_HELPER_PLIST_PATH),
            plist_contents().as_bytes(),
        ) {
            Ok(path) => path,
            Err(error) => {
                discard_staged(&[&staged_binary]);
                return Err(error);
            }
        };
        (staged_binary, staged_plist, candidate_hash)
    };
    if replace_legacy_helper {
        drop(mutation_lock);
        if let Err(error) = wait_for_legacy_upgrade(config_dir, data_dir) {
            discard_staged(&[&staged_binary, &staged_plist]);
            return Err(error);
        }
        mutation_lock = match mount_helper::acquire_mutation_lock() {
            Ok(lock) => lock,
            Err(error) => {
                discard_staged(&[&staged_binary, &staged_plist]);
                return Err(CliError::retryable(
                    "snapshot_mount_helper.lock_unavailable",
                    format!(
                        "cannot resume helper installation safely after Snapshot Access cleanup: {error}"
                    ),
                ));
            }
        };
        let refreshed_access = match snapshot_access_status_for_maintenance(
            config_dir,
            data_dir,
            "replacing the legacy snapshot mount helper",
        ) {
            Ok(status) => status,
            Err(error) => {
                discard_staged(&[&staged_binary, &staged_plist]);
                return Err(error);
            }
        };
        if let Err(error) = ensure_snapshot_access_idle(&refreshed_access) {
            discard_staged(&[&staged_binary, &staged_plist]);
            return Err(error);
        }
        let refreshed_busy = match check_if_busy("install or update the snapshot mount helper") {
            Ok(busy) => busy,
            Err(error) => {
                discard_staged(&[&staged_binary, &staged_plist]);
                return Err(error);
            }
        };
        if refreshed_busy == BusyCheck::RecoveryRequired {
            discard_staged(&[&staged_binary, &staged_plist]);
            return Err(CliError::retryable(
                "snapshot_mount_helper.status_unavailable",
                "snapshot mount helper became unavailable while waiting for Snapshot Access cleanup",
            ));
        }
    }
    let service = system_service_target();
    if let Err(error) =
        bootout_for_mutation(&service, "install", "snapshot_mount_helper.install_failed")
    {
        discard_staged(&[&staged_binary, &staged_plist]);
        return Err(error);
    }
    let (old_binary, old_plist) = match commit_staged_assets(&staged_binary, &staged_plist) {
        Ok(backups) => backups,
        Err(error) => {
            discard_staged(&[&staged_binary, &staged_plist]);
            drop(mutation_lock);
            return match bootstrap_restored_helper() {
                Ok(()) => Err(error),
                Err(bootstrap_error) => Err(rollback_failure(&error, bootstrap_error)),
            };
        }
    };
    drop(mutation_lock);
    let install_result = (|| {
        launchctl(&["bootstrap", "system", MOUNT_HELPER_PLIST_PATH])?;
        let started = Instant::now();
        let status = wait_for_installed_helper(
            env!("TELEVYBACKUP_BUILD_COMMIT"),
            mount_helper::status,
            std::thread::sleep,
            || started.elapsed(),
        )?;
        if helper_sha256(Path::new(MOUNT_HELPER_INSTALL_PATH))? != candidate_hash {
            return Err(CliError::new(
                "snapshot_mount_helper.identity_failed",
                "installed mount helper does not match the candidate binary",
            ));
        }
        Ok(status)
    })();
    let status = match install_result {
        Ok(status) => status,
        Err(error) => {
            return match rollback_installed_assets(old_binary, old_plist) {
                Ok(()) => Err(error),
                Err(rollback) => Err(rollback_failure(&error, rollback)),
            };
        }
    };
    if let Some(path) = old_binary {
        let _ = fs::remove_file(path);
    }
    if let Some(path) = old_plist {
        let _ = fs::remove_file(path);
    }
    if json_output {
        println!(
            "{}",
            json!({"installed": true, "helperReady": true, "helperVersion": status.component_version.as_deref().unwrap_or(&status.helper_version), "buildCommit": status.build_commit, "binarySha256": candidate_hash, "label": MOUNT_HELPER_LABEL, "binary": MOUNT_HELPER_INSTALL_PATH, "plist": MOUNT_HELPER_PLIST_PATH})
        );
    } else {
        println!("snapshot mount helper installed");
    }
    Ok(())
}

pub fn uninstall(config_dir: &Path, data_dir: &Path, json_output: bool) -> Result<(), CliError> {
    require_root()?;
    let _install_lock = mount_helper::acquire_install_lock().map_err(|error| {
        CliError::retryable(
            "snapshot_mount_helper.install_lock_unavailable",
            format!("cannot uninstall the snapshot mount helper safely: {error}"),
        )
    })?;
    let mut mutation_lock = mount_helper::acquire_mutation_lock().map_err(|error| {
        CliError::retryable(
            "snapshot_mount_helper.lock_unavailable",
            format!("cannot uninstall the snapshot mount helper safely: {error}"),
        )
    })?;
    recover_interrupted_helper_transaction(config_dir, data_dir)?;
    let mut busy = check_if_busy("uninstall the snapshot mount helper")?;
    if busy == BusyCheck::RecoveryRequired {
        drop(mutation_lock);
        recover_helper_before_mutation()?;
        mutation_lock = mount_helper::acquire_mutation_lock().map_err(|error| {
            CliError::retryable(
                "snapshot_mount_helper.lock_unavailable",
                format!("cannot resume helper removal safely after recovery: {error}"),
            )
        })?;
        busy = check_if_busy("uninstall the snapshot mount helper")?;
    }
    recover_interrupted_helper_transaction(config_dir, data_dir)?;
    ensure_uninstall_safe(busy)?;
    let service = system_service_target();
    bootout_for_mutation(
        &service,
        "uninstall",
        "snapshot_mount_helper.uninstall_failed",
    )?;
    let (old_binary, old_plist) = match quarantine_uninstall_assets() {
        Ok(backups) => backups,
        Err(error) => {
            drop(mutation_lock);
            return match bootstrap_restored_helper() {
                Ok(()) => Err(error),
                Err(bootstrap_error) => Err(rollback_failure(&error, bootstrap_error)),
            };
        }
    };
    drop(mutation_lock);
    // Quarantined files are root-owned and no longer reachable from the launchd contract. Best
    // effort cleanup is safe here; retaining one does not make the helper serviceable.
    if let Some(path) = old_binary {
        let _ = fs::remove_file(path);
    }
    if let Some(path) = old_plist {
        let _ = fs::remove_file(path);
    }
    if json_output {
        println!(
            "{}",
            json!({"uninstalled": true, "label": MOUNT_HELPER_LABEL, "journalPreserved": true})
        );
    } else {
        println!("snapshot mount helper uninstalled; journal preserved");
    }
    Ok(())
}

/// Retry only the helper's journal-bound cleanup-pending rows.
///
/// This is intentionally a separate explicit command. Status probes and refreshes remain
/// read-only, while each release still revalidates UID, volume/device identity and inventory in
/// the root helper before changing the journal.
#[cfg(not(unix))]
pub fn reconcile(config_dir: &Path, data_dir: &Path, json_output: bool) -> Result<(), CliError> {
    let uid = unsafe { libc::geteuid() };
    if uid == 0 {
        return Err(CliError::new(
            "snapshot_mount_helper.invalid_owner",
            "snapshot cleanup retry must run as the signed-in user so the root helper can verify the peer UID",
        ));
    }
    wait_for_snapshot_access_idle(config_dir, data_dir, "retrying snapshot cleanup")?;
    let status = wait_for_helper_status()?;
    cleanup_retry_identity(&status)?;
    let live_mounts = status.live_mounts.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot retry snapshot cleanup: live mount classification is unavailable",
        )
    })?;
    let recoverable = status.recoverable_stale_mounts.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot retry snapshot cleanup: stale mount classification is unavailable",
        )
    })?;
    let unknown = status.unknown_blocking_mounts.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot retry snapshot cleanup: unknown mount classification is unavailable",
        )
    })?;
    if live_mounts > 0 || recoverable > 0 || unknown > 0 {
        return Err(CliError::retryable(
            "snapshot_mount_helper.reconcile_required",
            format!(
                "cannot retry cleanup while other snapshot records remain (live: {live_mounts}, recoverable: {recoverable}, unknown: {unknown})"
            ),
        ));
    }
    let leases = pending_cleanup_leases(&status)?;
    let mut released = 0_u32;
    for lease_id in leases {
        let result = mount_helper::release(uid, &lease_id).map_err(|_| {
            CliError::retryable(
                "snapshot_mount_helper.reconcile_failed",
                "a pending snapshot cleanup record could not be safely retried",
            )
        })?;
        if result.cleanup_state == "released" {
            released += 1;
        }
    }
    let final_status = wait_for_helper_status()?;
    let final_pending = final_status.pending_cleanup.unwrap_or(u32::MAX);
    let final_live = final_status.live_mounts.unwrap_or(u32::MAX);
    let final_recoverable = final_status.recoverable_stale_mounts.unwrap_or(u32::MAX);
    let final_unknown = final_status.unknown_blocking_mounts.unwrap_or(u32::MAX);
    if final_live > 0 || final_recoverable > 0 || final_unknown > 0 {
        return Err(CliError::retryable(
            "snapshot_mount_helper.reconcile_required",
            format!(
                "snapshot cleanup retry left other snapshot records (live: {final_live}, recoverable: {final_recoverable}, unknown: {final_unknown})"
            ),
        ));
    }
    if final_pending > 0 {
        return Err(CliError::retryable(
            "snapshot_mount_helper.pending_cleanup",
            format!(
                "snapshot cleanup remains pending because the recorded snapshot inventory is not yet reconciled (released: {released}, remaining: {final_pending})"
            ),
        ));
    }
    if final_status.active_mounts != 0 {
        return Err(CliError::retryable(
            "snapshot_mount_helper.status_inconsistent",
            "snapshot cleanup retry returned a non-empty aggregate after all classified rows were released",
        ));
    }
    let payload = json!({
        "reconciled": true,
        "released": released,
        "remaining": final_status.active_mounts,
        "helperVersion": final_status.component_version.as_deref().unwrap_or(&final_status.helper_version),
        "buildCommit": final_status.build_commit,
    });
    if json_output {
        println!("{payload}");
    } else {
        println!("snapshot mount cleanup reconciled ({released} record(s) released)");
    }
    Ok(())
}

#[cfg(unix)]
pub fn reconcile(_config_dir: &Path, data_dir: &Path, json_output: bool) -> Result<(), CliError> {
    reconcile_cleanup_via_daemon(data_dir, json_output)
}

#[cfg(unix)]
fn reconcile_cleanup_via_daemon(data_dir: &Path, json_output: bool) -> Result<(), CliError> {
    let response = super::control_ipc_call_with_timeouts(
        data_dir,
        "snapshot.mount_helper.retry_cleanup",
        json!({}),
        Duration::from_secs(15),
        Duration::from_secs(10),
    )?;
    let operation_id = response
        .result
        .as_ref()
        .and_then(|value| value.get("operationId"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CliError::retryable(
                "snapshot_mount_helper.reconcile_failed",
                "daemon did not return a snapshot cleanup operation",
            )
        })?
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(15 * 60);
    loop {
        let status_response = super::control_ipc_call_with_timeouts(
            data_dir,
            "operation.get",
            json!({"operationId": operation_id}),
            Duration::from_secs(5),
            Duration::from_secs(5),
        )?;
        let status = status_response.result.ok_or_else(|| {
            CliError::retryable(
                "snapshot_mount_helper.reconcile_failed",
                "daemon returned no snapshot cleanup operation status",
            )
        })?;
        match status.get("state").and_then(serde_json::Value::as_str) {
            Some("succeeded") => {
                let mut payload = status.get("result").cloned().unwrap_or_else(|| json!({}));
                if let Some(object) = payload.as_object_mut() {
                    object.insert("reconciled".to_string(), json!(true));
                }
                if json_output {
                    println!("{payload}");
                } else {
                    let released = payload
                        .get("released")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default();
                    println!("snapshot mount cleanup reconciled ({released} record(s) released)");
                }
                return Ok(());
            }
            Some("failed") => {
                let error = status
                    .get("error")
                    .cloned()
                    .and_then(decode_control_error)
                    .unwrap_or_else(|| televy_backup_core::control::ControlError {
                        code: "snapshot_mount_helper.reconcile_failed".to_string(),
                        message: "snapshot cleanup reconciliation failed".to_string(),
                        retryable: true,
                        details: json!({}),
                    });
                let code = match error.code.as_str() {
                    "snapshot_mount_helper.active" => "snapshot_mount_helper.active",
                    "snapshot_mount_helper.pending_cleanup" => {
                        "snapshot_mount_helper.pending_cleanup"
                    }
                    "snapshot_mount_helper.reconcile_required" => {
                        "snapshot_mount_helper.reconcile_required"
                    }
                    "snapshot_mount_helper.status_incomplete" => {
                        "snapshot_mount_helper.status_incomplete"
                    }
                    "snapshot_mount_helper.status_inconsistent" => {
                        "snapshot_mount_helper.status_inconsistent"
                    }
                    "snapshot_mount_helper.update_required" => {
                        "snapshot_mount_helper.update_required"
                    }
                    _ => "snapshot_mount_helper.reconcile_failed",
                };
                let message = if error.message.is_empty() {
                    "snapshot cleanup reconciliation failed".to_string()
                } else {
                    error.message.clone()
                };
                let cli_error = if error.retryable {
                    CliError::retryable(code, message)
                } else {
                    CliError::new(code, message)
                };
                return Err(cli_error.with_details(error.details));
            }
            Some("pending" | "running") if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Some("pending" | "running") => {
                return Err(CliError::retryable(
                    "snapshot_mount_helper.reconcile_timeout",
                    "snapshot cleanup reconciliation did not finish before the timeout",
                ));
            }
            _ => {
                return Err(CliError::retryable(
                    "snapshot_mount_helper.reconcile_failed",
                    "daemon returned an invalid snapshot cleanup operation state",
                ));
            }
        }
    }
}

/// Explicitly re-baseline released tombstones whose only verified difference is the current
/// snapshot inventory. This never deletes snapshots or opens source files.
#[cfg(not(unix))]
pub fn reconcile_inventory(
    config_dir: &Path,
    data_dir: &Path,
    json_output: bool,
) -> Result<(), CliError> {
    let uid = unsafe { libc::geteuid() };
    if uid == 0 {
        return Err(CliError::new(
            "snapshot_mount_helper.invalid_owner",
            "snapshot inventory reconciliation must run as the signed-in user so the root helper can verify the peer UID",
        ));
    }
    wait_for_snapshot_access_idle(config_dir, data_dir, "reconciling snapshot inventory")?;
    let status = wait_for_helper_status()?;
    cleanup_retry_identity(&status)?;
    let live = status.live_mounts.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot reconcile snapshot inventory: live mount classification is unavailable",
        )
    })?;
    let pending = status.pending_cleanup.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot reconcile snapshot inventory: cleanup classification is unavailable",
        )
    })?;
    let recoverable = status.recoverable_stale_mounts.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot reconcile snapshot inventory: stale mount classification is unavailable",
        )
    })?;
    let unknown = status.unknown_blocking_mounts.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot reconcile snapshot inventory: unknown mount classification is unavailable",
        )
    })?;
    let drift = status.inventory_drift_mounts.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot reconcile snapshot inventory: inventory-drift classification is unavailable",
        )
    })?;
    if live > 0 || pending > 0 || recoverable > 0 || unknown > 0 {
        return Err(CliError::retryable(
            "snapshot_mount_helper.reconcile_required",
            format!(
                "cannot reconcile snapshot inventory while other records remain (live: {live}, pending: {pending}, recoverable: {recoverable}, unknown: {unknown})"
            ),
        ));
    }
    if status.active_mounts != drift
        || drift as usize != status.inventory_drift_leases.len()
        || status
            .inventory_drift_leases
            .iter()
            .any(|lease| lease.state != "released_inventory_drift")
    {
        return Err(CliError::retryable(
            "snapshot_mount_helper.status_inconsistent",
            "cannot reconcile snapshot inventory: the helper reported an inconsistent drift inventory",
        ));
    }
    let mut reconciled = 0_u32;
    for lease in &status.inventory_drift_leases {
        let lease_id = lease.lease_id.clone();
        let result = mount_helper::reconcile_inventory(uid, &lease_id).map_err(|_| {
            CliError::retryable(
                "snapshot_mount_helper.reconcile_failed",
                "a snapshot inventory record could not be safely reconciled",
            )
        })?;
        if result.cleanup_state == "released" {
            reconciled += 1;
        }
    }
    let final_status = wait_for_helper_status()?;
    let final_drift = final_status.inventory_drift_mounts.unwrap_or(u32::MAX);
    let final_unknown = final_status.unknown_blocking_mounts.unwrap_or(u32::MAX);
    if final_drift > 0 || final_unknown > 0 || final_status.active_mounts != 0 {
        return Err(CliError::retryable(
            "snapshot_mount_helper.reconcile_required",
            "snapshot inventory reconciliation did not clear every verified drift record",
        ));
    }
    let payload = json!({
        "reconciled": true,
        "records": reconciled,
        "remaining": final_status.active_mounts,
        "inventoryDrift": final_drift,
        "helperVersion": final_status.component_version.as_deref().unwrap_or(&final_status.helper_version),
        "buildCommit": final_status.build_commit,
    });
    if json_output {
        println!("{payload}");
    } else {
        println!("snapshot inventory reconciled ({reconciled} record(s))");
    }
    Ok(())
}

#[cfg(unix)]
pub fn reconcile_inventory(
    _config_dir: &Path,
    data_dir: &Path,
    json_output: bool,
) -> Result<(), CliError> {
    reconcile_inventory_via_daemon(data_dir, json_output)
}

#[cfg(unix)]
fn reconcile_inventory_via_daemon(data_dir: &Path, json_output: bool) -> Result<(), CliError> {
    let response = super::control_ipc_call_with_timeouts(
        data_dir,
        "snapshot.mount_helper.reconcile_inventory",
        json!({}),
        Duration::from_secs(15),
        Duration::from_secs(10),
    )?;
    let operation_id = response
        .result
        .as_ref()
        .and_then(|value| value.get("operationId"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CliError::retryable(
                "snapshot_mount_helper.reconcile_failed",
                "daemon did not return a snapshot inventory reconciliation operation",
            )
        })?
        .to_owned();
    let deadline = Instant::now() + Duration::from_secs(15 * 60);
    loop {
        let status_response = super::control_ipc_call_with_timeouts(
            data_dir,
            "operation.get",
            json!({"operationId": operation_id}),
            Duration::from_secs(5),
            Duration::from_secs(5),
        )?;
        let status = status_response.result.ok_or_else(|| {
            CliError::retryable(
                "snapshot_mount_helper.reconcile_failed",
                "daemon returned no reconciliation operation status",
            )
        })?;
        match status.get("state").and_then(serde_json::Value::as_str) {
            Some("succeeded") => {
                let mut payload = status.get("result").cloned().unwrap_or_else(|| json!({}));
                if let Some(object) = payload.as_object_mut() {
                    let reconciled = object
                        .get("reconciled")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default();
                    object.insert("reconciled".to_string(), json!(true));
                    object.insert("records".to_string(), json!(reconciled));
                }
                if json_output {
                    println!("{payload}");
                } else {
                    let records = payload
                        .get("records")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or_default();
                    println!("snapshot inventory reconciled ({records} record(s))");
                }
                return Ok(());
            }
            Some("failed") => {
                let error = status
                    .get("error")
                    .cloned()
                    .and_then(decode_control_error)
                    .unwrap_or_else(|| televy_backup_core::control::ControlError {
                        code: "snapshot_mount_helper.reconcile_failed".to_string(),
                        message: "snapshot inventory reconciliation failed".to_string(),
                        retryable: true,
                        details: json!({}),
                    });
                let (code, fallback_code) = match error.code.as_str() {
                    "snapshot_mount_helper.active" => (
                        "snapshot_mount_helper.active",
                        "snapshot inventory reconciliation is blocked by an active mount",
                    ),
                    "snapshot_mount_helper.pending_cleanup" => (
                        "snapshot_mount_helper.pending_cleanup",
                        "snapshot inventory reconciliation is blocked by pending cleanup",
                    ),
                    "snapshot_mount_helper.unknown_blocker" => (
                        "snapshot_mount_helper.unknown_blocker",
                        "snapshot inventory reconciliation is blocked by an unknown record",
                    ),
                    "snapshot_mount_helper.reconcile_required" => (
                        "snapshot_mount_helper.reconcile_required",
                        "snapshot inventory reconciliation remains blocked",
                    ),
                    _ => (
                        "snapshot_mount_helper.reconcile_failed",
                        "snapshot inventory reconciliation failed",
                    ),
                };
                let message = if error.message.is_empty() {
                    fallback_code
                } else {
                    error.message.as_str()
                };
                let cli_error = if error.retryable {
                    CliError::retryable(code, message)
                } else {
                    CliError::new(code, message)
                };
                return Err(cli_error.with_details(error.details));
            }
            Some("pending" | "running") if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Some("pending" | "running") => {
                return Err(CliError::retryable(
                    "snapshot_mount_helper.reconcile_timeout",
                    "snapshot inventory reconciliation did not finish before the timeout",
                ));
            }
            _ => {
                return Err(CliError::retryable(
                    "snapshot_mount_helper.reconcile_failed",
                    "daemon returned an invalid reconciliation operation state",
                ));
            }
        }
    }
}

#[cfg(unix)]
fn decode_control_error(
    value: serde_json::Value,
) -> Option<televy_backup_core::control::ControlError> {
    serde_json::from_value(value).ok()
}

pub fn status(json_output: bool) -> Result<(), CliError> {
    let loaded = helper_service_state() == HelperServiceState::Loaded;
    let helper = mount_helper::status().ok();
    let payload = json!({
        "installed": safe_root_asset(Path::new(MOUNT_HELPER_INSTALL_PATH))
            && safe_root_asset(Path::new(MOUNT_HELPER_PLIST_PATH)),
        "launchdLoaded": loaded,
        "label": MOUNT_HELPER_LABEL,
        "activeMounts": helper.as_ref().map(|status| status.active_mounts),
        "pendingCleanup": helper.as_ref().and_then(|status| status.pending_cleanup),
        "liveMounts": helper.as_ref().and_then(|status| status.live_mounts),
        "recoverableStaleMounts": helper.as_ref().and_then(|status| status.recoverable_stale_mounts),
        "unknownBlockingMounts": helper.as_ref().and_then(|status| status.unknown_blocking_mounts),
        "inventoryDriftMounts": helper.as_ref().and_then(|status| status.inventory_drift_mounts),
        "helperVersion": helper.as_ref().map(|status| status.component_version.clone().unwrap_or_else(|| status.helper_version.clone())),
        "buildCommit": helper.as_ref().and_then(|status| status.build_commit.clone()),
        "socket": mount_helper::configured_socket_path(),
    });
    if json_output {
        println!("{payload}");
    } else {
        println!(
            "snapshot mount helper: {}",
            if loaded { "loaded" } else { "not loaded" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    const CANDIDATE_COMMIT: &str = "0123456789012345678901234567890123456789";

    fn candidate_status() -> MountStatusResult {
        MountStatusResult {
            helper_version: mount_helper::ROOT_MOUNT_HELPER_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some(CANDIDATE_COMMIT.into()),
            ..Default::default()
        }
    }

    #[test]
    fn busy_error_distinguishes_pending_cleanup_from_active_mounts() {
        let pending = MountStatusResult {
            active_mounts: 1,
            pending_cleanup: Some(1),
            ..Default::default()
        };
        let error = busy_error("update the snapshot mount helper", &pending).unwrap();
        assert_eq!(error.code, "snapshot_mount_helper.cleanup_pending");
        assert!(
            error
                .message
                .contains("no active snapshot mount is running")
        );

        let active = MountStatusResult {
            active_mounts: 1,
            pending_cleanup: Some(0),
            ..Default::default()
        };
        let error = busy_error("update the snapshot mount helper", &active).unwrap();
        assert_eq!(error.code, "snapshot_mount_helper.busy");
        assert!(error.message.contains("1 active snapshot mount(s)"));

        let mixed = MountStatusResult {
            active_mounts: 2,
            pending_cleanup: Some(1),
            ..Default::default()
        };
        let error = busy_error("update the snapshot mount helper", &mixed).unwrap();
        assert_eq!(error.code, "snapshot_mount_helper.busy");
        assert!(error.message.contains("1 active snapshot mount(s)"));
        assert!(error.message.contains("1 pending cleanup record(s)"));
    }

    #[test]
    fn busy_error_distinguishes_released_inventory_drift() {
        let status = MountStatusResult {
            active_mounts: 1,
            pending_cleanup: Some(0),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: Some(1),
            ..Default::default()
        };
        let error = busy_error("start a strict backup", &status).unwrap();
        assert_eq!(error.code, "snapshot_mount_helper.inventory_drift");
        assert!(error.message.contains("explicit reconciliation"));
    }

    #[test]
    fn missing_cleanup_status_remains_blocking() {
        let error = busy_error(
            "update the snapshot mount helper",
            &MountStatusResult {
                active_mounts: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(error.code, "snapshot_mount_helper.status_incomplete");
    }

    #[test]
    fn pending_cleanup_candidates_require_a_complete_pending_only_partition() {
        let status = MountStatusResult {
            active_mounts: 1,
            pending_cleanup: Some(1),
            pending_cleanup_leases: vec![televybackup_snapshot_access::MountRecoveryLease {
                lease_id: "pending".into(),
                state: "cleanup_pending".into(),
            }],
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: Some(0),
            ..Default::default()
        };
        assert_eq!(pending_cleanup_leases(&status).unwrap(), vec!["pending"]);

        let mut wrong_state = status.clone();
        wrong_state.pending_cleanup_leases[0].state = "active".into();
        assert_eq!(
            pending_cleanup_leases(&wrong_state).unwrap_err().code,
            "snapshot_mount_helper.status_inconsistent"
        );

        let mut mixed = status.clone();
        mixed.active_mounts = 2;
        assert_eq!(
            pending_cleanup_leases(&mixed).unwrap_err().code,
            "snapshot_mount_helper.status_inconsistent"
        );

        let mut drift = status;
        drift.inventory_drift_mounts = Some(1);
        drift.active_mounts = 2;
        assert_eq!(
            pending_cleanup_leases(&drift).unwrap_err().code,
            "snapshot_mount_helper.status_inconsistent"
        );
    }

    #[test]
    fn cleanup_retry_accepts_the_previous_cleanup_capable_helper() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::PREVIOUS_ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            ..Default::default()
        };
        assert!(cleanup_retry_identity(&status).is_ok());
    }

    #[test]
    fn legacy_pending_only_status_requires_a_guarded_upgrade() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::PREVIOUS_ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            active_mounts: 1,
            pending_cleanup: Some(1),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: Some(0),
            ..Default::default()
        };
        assert_eq!(
            classify_busy_status("install or update", &status).unwrap(),
            BusyCheck::LegacyUpgrade
        );
    }

    #[test]
    fn legacy_pending_only_status_without_inventory_drift_field_allows_upgrade() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::PREVIOUS_ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            active_mounts: 1,
            pending_cleanup: Some(1),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: None,
            ..Default::default()
        };
        assert_eq!(
            classify_busy_status("install or update", &status).unwrap(),
            BusyCheck::LegacyUpgrade
        );
    }

    #[test]
    fn current_helper_pending_only_with_stale_build_enters_guarded_upgrade() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            active_mounts: 1,
            pending_cleanup: Some(1),
            pending_cleanup_leases: vec![televybackup_snapshot_access::MountRecoveryLease {
                lease_id: "pending".into(),
                state: "cleanup_pending".into(),
            }],
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: Some(0),
            ..Default::default()
        };
        assert_eq!(
            classify_busy_status("install or update", &status).unwrap(),
            BusyCheck::PendingCleanupUpgrade
        );
    }

    #[test]
    fn current_helper_pending_only_with_candidate_build_stays_cleanup_blocked() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some(env!("TELEVYBACKUP_BUILD_COMMIT").into()),
            active_mounts: 1,
            pending_cleanup: Some(1),
            pending_cleanup_leases: vec![televybackup_snapshot_access::MountRecoveryLease {
                lease_id: "pending".into(),
                state: "cleanup_pending".into(),
            }],
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: Some(0),
            ..Default::default()
        };
        let error = classify_busy_status("install or update", &status).unwrap_err();
        assert_eq!(error.code, "snapshot_mount_helper.cleanup_pending");
    }

    #[test]
    fn current_helper_pending_only_without_exact_candidates_stays_blocked() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            active_mounts: 1,
            pending_cleanup: Some(1),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: Some(0),
            ..Default::default()
        };
        let error = classify_busy_status("install or update", &status).unwrap_err();
        assert_eq!(error.code, "snapshot_mount_helper.cleanup_pending");
    }

    #[test]
    fn known_legacy_status_with_build_identity_enters_upgrade_recovery_path() {
        for helper_version in [
            mount_helper::PREVIOUS_ROOT_MOUNT_HELPER_VERSION,
            mount_helper::LEGACY_ROOT_MOUNT_HELPER_VERSION,
            mount_helper::HISTORICAL_ROOT_MOUNT_HELPER_VERSION,
        ] {
            let status = MountStatusResult {
                helper_version: helper_version.into(),
                active_mounts: 1,
                build_commit: Some(CANDIDATE_COMMIT.into()),
                ..Default::default()
            };
            assert_eq!(
                classify_busy_status("install or update the snapshot mount helper", &status)
                    .unwrap(),
                BusyCheck::LegacyUpgrade
            );
        }
    }

    #[test]
    fn current_helper_without_classification_remains_incomplete() {
        let status = MountStatusResult {
            helper_version: mount_helper::ROOT_MOUNT_HELPER_VERSION.into(),
            active_mounts: 1,
            build_commit: Some(CANDIDATE_COMMIT.into()),
            ..Default::default()
        };
        let error = classify_busy_status("install or update the snapshot mount helper", &status)
            .unwrap_err();
        assert_eq!(error.code, "snapshot_mount_helper.status_incomplete");
    }

    #[test]
    fn stale_current_helper_without_inventory_classification_enters_guarded_upgrade() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            active_mounts: 1,
            pending_cleanup: Some(0),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(1),
            inventory_drift_mounts: None,
            ..Default::default()
        };
        assert_eq!(
            classify_busy_status("install or update", &status).unwrap(),
            BusyCheck::LegacyUpgrade
        );
    }

    #[test]
    fn stale_current_helper_with_zero_rows_enters_guarded_upgrade() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            active_mounts: 0,
            pending_cleanup: Some(0),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: None,
            ..Default::default()
        };
        assert_eq!(
            classify_busy_status("install or update", &status).unwrap(),
            BusyCheck::LegacyUpgrade
        );
    }

    #[test]
    fn current_candidate_without_inventory_classification_stays_incomplete() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some(env!("TELEVYBACKUP_BUILD_COMMIT").into()),
            active_mounts: 1,
            pending_cleanup: Some(0),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(1),
            inventory_drift_mounts: None,
            ..Default::default()
        };
        let error = classify_busy_status("install or update", &status).unwrap_err();
        assert_eq!(error.code, "snapshot_mount_helper.status_incomplete");
    }

    #[test]
    fn stale_current_helper_requires_the_stable_wire_identity() {
        let mut status = MountStatusResult {
            helper_version: mount_helper::ROOT_MOUNT_HELPER_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            active_mounts: 1,
            pending_cleanup: Some(0),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(1),
            inventory_drift_mounts: None,
            ..Default::default()
        };
        let error = classify_busy_status("install or update", &status).unwrap_err();
        assert_eq!(error.code, "snapshot_mount_helper.status_incomplete");

        status.helper_version = mount_helper::MOUNT_HELPER_WIRE_VERSION.into();
        assert_eq!(
            classify_busy_status("install or update", &status).unwrap(),
            BusyCheck::LegacyUpgrade
        );
    }

    #[test]
    fn stale_current_helper_rejects_an_inconsistent_known_partition() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            build_commit: Some("b".repeat(40)),
            active_mounts: 1,
            pending_cleanup: Some(1),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(1),
            inventory_drift_mounts: None,
            ..Default::default()
        };
        let error = classify_busy_status("install or update", &status).unwrap_err();
        assert_eq!(error.code, "snapshot_mount_helper.status_incomplete");
    }

    #[test]
    fn legacy_upgrade_requires_snapshot_access_to_be_idle() {
        let mut status = StatusResult::default();
        assert!(ensure_snapshot_access_idle(&status).is_ok());

        status.active_leases = 1;
        let error = ensure_snapshot_access_idle(&status).unwrap_err();
        assert_eq!(error.code, "snapshot_mount_helper.active");

        status.active_leases = 0;
        status.pending_cleanup = 1;
        let error = ensure_snapshot_access_idle(&status).unwrap_err();
        assert_eq!(error.code, "snapshot_mount_helper.access_cleanup_pending");
    }

    #[test]
    fn uninstall_rejects_any_non_clear_mount_status() {
        assert!(ensure_uninstall_safe(BusyCheck::Clear).is_ok());
        for check in [BusyCheck::LegacyUpgrade, BusyCheck::PendingCleanupUpgrade] {
            assert_eq!(
                ensure_uninstall_safe(check).unwrap_err().code,
                "snapshot_mount_helper.busy"
            );
        }
        assert_eq!(
            ensure_uninstall_safe(BusyCheck::RecoveryRequired)
                .unwrap_err()
                .code,
            "snapshot_mount_helper.status_unavailable"
        );
    }

    #[test]
    fn uninstall_operation_is_distinct_from_installation() {
        assert!(is_uninstall_operation(
            "uninstall the snapshot mount helper"
        ));
        assert!(!is_uninstall_operation(
            "install or update the snapshot mount helper"
        ));
    }

    #[test]
    fn missing_helper_assets_block_uninstall_but_allow_first_install() {
        assert_eq!(
            no_assets_busy_check("uninstall the snapshot mount helper")
                .unwrap_err()
                .code,
            "snapshot_mount_helper.status_unavailable"
        );
        assert_eq!(
            no_assets_busy_check("install or update the snapshot mount helper").unwrap(),
            BusyCheck::Clear
        );
    }

    #[test]
    fn uninstall_ignores_only_explicit_service_absence() {
        let absent = CliError::retryable(
            "snapshot_mount_helper.launchctl_failed",
            "Could not find service \"system/com.ivan.televybackup.snapshot-mount-helper\"",
        );
        assert!(launchctl_service_absent(&absent));

        let unknown = CliError::retryable(
            "snapshot_mount_helper.launchctl_failed",
            "Operation not permitted",
        );
        assert!(!launchctl_service_absent(&unknown));
    }

    #[test]
    fn non_absent_bootout_failure_is_returned_before_asset_mutation() {
        let error = bootout_failure(
            CliError::retryable(
                "snapshot_mount_helper.launchctl_failed",
                "Operation not permitted",
            ),
            "install",
            "snapshot_mount_helper.install_failed",
        );
        assert_eq!(error.code, "snapshot_mount_helper.install_failed");
        assert!(error.message.contains("Operation not permitted"));
    }

    #[test]
    fn legacy_zero_aggregate_allows_a_verified_replacement() {
        assert!(
            busy_error(
                "update the snapshot mount helper",
                &MountStatusResult {
                    active_mounts: 0,
                    pending_cleanup: None,
                    ..Default::default()
                }
            )
            .is_none()
        );
    }

    #[test]
    fn zero_aggregate_with_blocking_classification_is_not_treated_as_clear() {
        let status = MountStatusResult {
            active_mounts: 0,
            pending_cleanup: Some(1),
            ..Default::default()
        };
        let error = busy_error("update the snapshot mount helper", &status).unwrap();
        assert_eq!(error.code, "snapshot_mount_helper.status_incomplete");
    }

    #[test]
    fn current_helper_zero_aggregate_requires_all_classification_fields() {
        let status = MountStatusResult {
            helper_version: mount_helper::MOUNT_HELPER_WIRE_VERSION.into(),
            component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
            active_mounts: 0,
            pending_cleanup: Some(0),
            live_mounts: Some(0),
            recoverable_stale_mounts: Some(0),
            unknown_blocking_mounts: Some(0),
            inventory_drift_mounts: None,
            ..Default::default()
        };
        let error = busy_error("update the snapshot mount helper", &status).unwrap();
        assert_eq!(error.code, "snapshot_mount_helper.status_incomplete");
    }

    #[test]
    fn plist_is_a_minimal_system_daemon() {
        let plist = plist_contents();
        assert!(plist.contains(MOUNT_HELPER_LABEL));
        assert!(plist.contains("mount-mount-helper") || plist.contains("snapshot-mount-helper"));
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
        assert!(!plist.contains("vault"));
        assert!(!plist.contains("Keychain"));
    }

    #[test]
    fn launchctl_uses_a_fully_qualified_system_service_target() {
        assert_eq!(
            system_service_target(),
            "system/com.ivan.televybackup.snapshot-mount-helper"
        );
    }

    #[test]
    fn launchctl_state_requires_explicit_service_absence_before_recovery() {
        assert_eq!(
            classify_helper_service_state(false, b"Could not find service"),
            HelperServiceState::Absent
        );
        assert_eq!(
            classify_helper_service_state(false, b"launchctl: unknown error"),
            HelperServiceState::Unknown
        );
        assert_eq!(
            classify_helper_service_state(true, b"Could not find service"),
            HelperServiceState::Loaded
        );
    }

    #[test]
    fn install_waits_for_the_new_component_before_acknowledging_success() {
        let mut calls = 0;
        let mut pauses = 0;
        let clock = Cell::new(Duration::ZERO);
        let result = wait_for_installed_helper(
            CANDIDATE_COMMIT,
            || {
                calls += 1;
                if calls == 1 {
                    return Err(mount_helper::MountHelperError::Unavailable(
                        "fixture startup".into(),
                    ));
                }
                Ok(candidate_status())
            },
            |duration| {
                pauses += 1;
                clock.set(clock.get() + duration);
            },
            || clock.get(),
        );
        assert!(result.is_ok());
        assert_eq!(calls, 2);
        assert_eq!(pauses, 1);
    }

    #[test]
    fn install_cannot_acknowledge_a_stale_or_unresponsive_service() {
        for stale in [true, false] {
            let mut calls = 0;
            let clock = Cell::new(Duration::ZERO);
            let error = wait_for_installed_helper(
                CANDIDATE_COMMIT,
                || {
                    calls += 1;
                    if stale {
                        Ok(MountStatusResult {
                            helper_version: mount_helper::LEGACY_ROOT_MOUNT_HELPER_VERSION.into(),
                            ..Default::default()
                        })
                    } else {
                        Err(mount_helper::MountHelperError::Protocol(
                            "unsupported mount helper response version".into(),
                        ))
                    }
                },
                |duration| clock.set(clock.get() + duration),
                || clock.get(),
            )
            .unwrap_err();
            assert_eq!(error.code, "snapshot_mount_helper.readiness_failed");
            assert!(error.message.contains("expected protocol 4"));
            assert!(error.message.contains("20s startup window"));
            assert!(calls > 4);
            assert_eq!(clock.get(), INSTALL_READY_TIMEOUT);
        }
    }

    #[test]
    fn install_rejects_same_component_with_stale_or_missing_build() {
        for build_commit in [
            None,
            Some("fedcba9876543210fedcba9876543210fedcba98".into()),
        ] {
            let clock = Cell::new(Duration::ZERO);
            let error = wait_for_installed_helper(
                CANDIDATE_COMMIT,
                || {
                    Ok(MountStatusResult {
                        helper_version: mount_helper::ROOT_MOUNT_HELPER_VERSION.into(),
                        component_version: Some(mount_helper::ROOT_MOUNT_HELPER_VERSION.into()),
                        build_commit: build_commit.clone(),
                        ..Default::default()
                    })
                },
                |duration| clock.set(clock.get() + duration),
                || clock.get(),
            )
            .unwrap_err();
            assert_eq!(error.code, "snapshot_mount_helper.readiness_failed");
        }
        let legacy: MountStatusResult =
            serde_json::from_str(r#"{"helper_version":"0.5.0","active_mounts":0}"#).unwrap();
        assert!(legacy.build_commit.is_none());
    }

    #[test]
    fn install_waits_through_connection_refusal_until_slow_startup_is_ready() {
        let clock = Cell::new(Duration::ZERO);
        let result = wait_for_installed_helper(
            CANDIDATE_COMMIT,
            || {
                if clock.get() < Duration::from_secs(2) {
                    Err(mount_helper::MountHelperError::Unavailable(
                        "Connection refused (os error 61)".into(),
                    ))
                } else {
                    Ok(candidate_status())
                }
            },
            |duration| clock.set(clock.get() + duration),
            || clock.get(),
        );
        assert!(
            result.is_ok(),
            "slow-starting helper must not exhaust four immediate refusals: {result:?}"
        );
        assert_eq!(clock.get(), Duration::from_secs(2));
    }

    #[test]
    fn install_bounds_permanent_connection_refusal_and_keeps_the_cause() {
        let clock = Cell::new(Duration::ZERO);
        let error = wait_for_installed_helper(
            CANDIDATE_COMMIT,
            || {
                assert!(clock.get() < INSTALL_READY_TIMEOUT);
                Err(mount_helper::MountHelperError::Unavailable(
                    "Connection refused (os error 61)".into(),
                ))
            },
            |duration| clock.set(clock.get() + duration),
            || clock.get(),
        )
        .unwrap_err();
        assert_eq!(clock.get(), INSTALL_READY_TIMEOUT);
        assert_eq!(error.code, "snapshot_mount_helper.readiness_failed");
        assert!(error.message.contains("Connection refused (os error 61)"));
    }

    #[test]
    fn install_counts_probe_time_and_never_accepts_a_late_ready_response() {
        let clock = Cell::new(Duration::ZERO);
        let mut calls = 0;
        let error = wait_for_installed_helper(
            CANDIDATE_COMMIT,
            || {
                calls += 1;
                clock.set(INSTALL_READY_TIMEOUT + Duration::from_millis(1));
                Ok(candidate_status())
            },
            |_| panic!("must not sleep after the startup deadline"),
            || clock.get(),
        )
        .unwrap_err();
        assert_eq!(calls, 1);
        assert_eq!(error.code, "snapshot_mount_helper.readiness_failed");
    }

    #[test]
    fn install_limits_the_last_pause_to_the_remaining_startup_window() {
        let clock = Cell::new(Duration::ZERO);
        let error = wait_for_installed_helper(
            CANDIDATE_COMMIT,
            || {
                clock.set(INSTALL_READY_TIMEOUT - Duration::from_millis(10));
                Err(mount_helper::MountHelperError::Unavailable(
                    "not listening yet".into(),
                ))
            },
            |duration| {
                assert_eq!(duration, Duration::from_millis(10));
                clock.set(clock.get() + duration);
            },
            || clock.get(),
        )
        .unwrap_err();
        assert_eq!(clock.get(), INSTALL_READY_TIMEOUT);
        assert_eq!(error.code, "snapshot_mount_helper.readiness_failed");
    }
}
