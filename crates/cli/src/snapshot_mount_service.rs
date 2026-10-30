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

fn atomic_copy(source: &Path, destination: &Path) -> Result<(), CliError> {
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
    fs::copy(source, &staged).map_err(|error| {
        CliError::new("snapshot_mount_helper.install_failed", error.to_string())
    })?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755)).map_err(|error| {
        CliError::new("snapshot_mount_helper.install_failed", error.to_string())
    })?;
    set_root_owner(&staged, "snapshot mount helper")?;
    fs::rename(&staged, destination)
        .map_err(|error| CliError::new("snapshot_mount_helper.install_failed", error.to_string()))
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), CliError> {
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
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&staged)
        .map_err(|error| {
            CliError::new("snapshot_mount_helper.install_failed", error.to_string())
        })?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| {
            CliError::new("snapshot_mount_helper.install_failed", error.to_string())
        })?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o644)).map_err(|error| {
        CliError::new("snapshot_mount_helper.install_failed", error.to_string())
    })?;
    set_root_owner(&staged, "snapshot mount helper plist")?;
    fs::rename(staged, path)
        .map_err(|error| CliError::new("snapshot_mount_helper.install_failed", error.to_string()))
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
        // The legacy aggregate includes every blocking journal row, including cleanup-pending
        // rows. A legacy helper that reports zero therefore has no row to protect, even though it
        // cannot provide the additive classification fields. The replacement transaction still
        // requires the current component and build identity before acknowledging readiness.
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

fn pending_cleanup_leases(status: &MountStatusResult) -> Result<Vec<String>, CliError> {
    let pending_cleanup = status.pending_cleanup.ok_or_else(|| {
        CliError::retryable(
            "snapshot_mount_helper.status_incomplete",
            "cannot retry snapshot cleanup: the installed snapshot mount helper did not report a complete cleanup status",
        )
    })?;
    if status.active_mounts != pending_cleanup
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
        && status.unknown_blocking_mounts.is_none();
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
        && status.pending_cleanup_leases.is_empty();
    if legacy_pending_only {
        return Ok(BusyCheck::LegacyUpgrade);
    }
    if status.active_mounts > 0 && known_legacy_wire && classification_missing {
        return Ok(BusyCheck::LegacyUpgrade);
    }
    busy_error(operation, status).map_or(Ok(BusyCheck::Clear), Err)
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

fn wait_for_snapshot_access_idle(
    config_dir: &Path,
    data_dir: &Path,
    operation: &str,
) -> Result<StatusResult, CliError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = super::snapshot_service::status_for_maintenance(config_dir, data_dir)
            .map_err(|error| {
                CliError::retryable(
                    "snapshot_mount_helper.access_unavailable",
                    format!(
                        "cannot verify Snapshot Access before {operation}: {}",
                        error.message
                    ),
                )
            })?;
        if status.active_leases > 0 || Instant::now() >= deadline {
            ensure_snapshot_access_idle(&status)?;
        }
        if status.pending_cleanup == 0 {
            return Ok(status);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn prepare_legacy_upgrade(config_dir: &Path, data_dir: &Path) -> Result<(), CliError> {
    wait_for_snapshot_access_idle(
        config_dir,
        data_dir,
        "replacing the legacy snapshot mount helper",
    )?;
    launchctl(&["bootout", &system_service_target()])
}

fn check_if_busy(operation: &str) -> Result<BusyCheck, CliError> {
    match mount_helper::status() {
        Ok(status) => classify_busy_status(operation, &status),
        Err(_error) if !helper_assets_are_installed() => Ok(BusyCheck::Clear),
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
    let mut mutation_lock = mount_helper::acquire_mutation_lock().map_err(|error| {
        CliError::retryable(
            "snapshot_mount_helper.lock_unavailable",
            format!("cannot install or update the snapshot mount helper safely: {error}"),
        )
    })?;
    let busy = check_if_busy("install or update the snapshot mount helper")?;
    match busy {
        BusyCheck::Clear => {}
        BusyCheck::LegacyUpgrade => prepare_legacy_upgrade(config_dir, data_dir)?,
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
                BusyCheck::LegacyUpgrade => prepare_legacy_upgrade(config_dir, data_dir)?,
                BusyCheck::RecoveryRequired => {
                    return Err(CliError::retryable(
                        "snapshot_mount_helper.status_unavailable",
                        "snapshot mount helper remained absent after recovery bootstrap",
                    ));
                }
            }
        }
    }
    let candidate_hash = {
        let source = current_executable_sibling()?;
        let candidate_hash = helper_sha256(&source)?;
        atomic_copy(&source, Path::new(MOUNT_HELPER_INSTALL_PATH))?;
        atomic_write(
            Path::new(MOUNT_HELPER_PLIST_PATH),
            plist_contents().as_bytes(),
        )?;
        let service = system_service_target();
        let _ = launchctl(&["bootout", &service]);
        candidate_hash
        // Release the shared lock before bootstrap: the new root helper must take it during
        // startup recovery before it can create the ready IPC socket.
    };
    drop(mutation_lock);
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

pub fn uninstall(json_output: bool) -> Result<(), CliError> {
    require_root()?;
    let mut mutation_lock = mount_helper::acquire_mutation_lock().map_err(|error| {
        CliError::retryable(
            "snapshot_mount_helper.lock_unavailable",
            format!("cannot uninstall the snapshot mount helper safely: {error}"),
        )
    })?;
    if check_if_busy("uninstall the snapshot mount helper")? == BusyCheck::RecoveryRequired {
        drop(mutation_lock);
        recover_helper_before_mutation()?;
        mutation_lock = mount_helper::acquire_mutation_lock().map_err(|error| {
            CliError::retryable(
                "snapshot_mount_helper.lock_unavailable",
                format!("cannot resume helper removal safely after recovery: {error}"),
            )
        })?;
        if check_if_busy("uninstall the snapshot mount helper")? == BusyCheck::RecoveryRequired {
            return Err(CliError::retryable(
                "snapshot_mount_helper.status_unavailable",
                "snapshot mount helper remained absent after recovery bootstrap",
            ));
        }
    }
    let service = system_service_target();
    let _ = launchctl(&["bootout", &service]);
    if Path::new(MOUNT_HELPER_PLIST_PATH).exists() {
        fs::remove_file(MOUNT_HELPER_PLIST_PATH).map_err(|error| {
            CliError::new("snapshot_mount_helper.uninstall_failed", error.to_string())
        })?;
    }
    if Path::new(MOUNT_HELPER_INSTALL_PATH).exists() {
        fs::remove_file(MOUNT_HELPER_INSTALL_PATH).map_err(|error| {
            CliError::new("snapshot_mount_helper.uninstall_failed", error.to_string())
        })?;
    }
    drop(mutation_lock);
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
            ..Default::default()
        };
        assert_eq!(pending_cleanup_leases(&status).unwrap(), vec!["pending"]);

        let mut wrong_state = status.clone();
        wrong_state.pending_cleanup_leases[0].state = "active".into();
        assert_eq!(
            pending_cleanup_leases(&wrong_state).unwrap_err().code,
            "snapshot_mount_helper.status_inconsistent"
        );

        let mut mixed = status;
        mixed.active_mounts = 2;
        assert_eq!(
            pending_cleanup_leases(&mixed).unwrap_err().code,
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
            ..Default::default()
        };
        assert_eq!(
            classify_busy_status("install or update", &status).unwrap(),
            BusyCheck::LegacyUpgrade
        );
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
