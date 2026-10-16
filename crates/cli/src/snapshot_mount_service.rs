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
    MOUNT_HELPER_INSTALL_PATH, MOUNT_HELPER_PROTOCOL_VERSION, MountStatusResult, mount_helper,
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

fn reject_if_busy(operation: &str) -> Result<(), CliError> {
    match mount_helper::status() {
        Ok(status) if status.active_mounts > 0 => Err(CliError::new(
            "snapshot_mount_helper.busy",
            format!(
                "cannot {operation} while {} snapshot mount(s) are active",
                status.active_mounts
            ),
        )),
        Ok(_) => Ok(()),
        Err(_) => Ok(()),
    }
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
                if status.helper_version == mount_helper::ROOT_MOUNT_HELPER_VERSION
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
                    status.helper_version,
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

pub fn install(json_output: bool) -> Result<(), CliError> {
    require_root()?;
    reject_if_busy("install or update the snapshot mount helper")?;
    let source = current_executable_sibling()?;
    let candidate_hash = helper_sha256(&source)?;
    atomic_copy(&source, Path::new(MOUNT_HELPER_INSTALL_PATH))?;
    atomic_write(
        Path::new(MOUNT_HELPER_PLIST_PATH),
        plist_contents().as_bytes(),
    )?;
    let service = system_service_target();
    let _ = launchctl(&["bootout", &service]);
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
            json!({"installed": true, "helperReady": true, "helperVersion": status.helper_version, "buildCommit": status.build_commit, "binarySha256": candidate_hash, "label": MOUNT_HELPER_LABEL, "binary": MOUNT_HELPER_INSTALL_PATH, "plist": MOUNT_HELPER_PLIST_PATH})
        );
    } else {
        println!("snapshot mount helper installed");
    }
    Ok(())
}

pub fn uninstall(json_output: bool) -> Result<(), CliError> {
    require_root()?;
    reject_if_busy("uninstall the snapshot mount helper")?;
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

pub fn status(json_output: bool) -> Result<(), CliError> {
    let loaded = Command::new("/bin/launchctl")
        .args(["print", &format!("system/{MOUNT_HELPER_LABEL}")])
        .output()
        .is_ok_and(|output| output.status.success());
    let helper = mount_helper::status().ok();
    let payload = json!({
        "installed": safe_root_asset(Path::new(MOUNT_HELPER_INSTALL_PATH))
            && safe_root_asset(Path::new(MOUNT_HELPER_PLIST_PATH)),
        "launchdLoaded": loaded,
        "label": MOUNT_HELPER_LABEL,
        "activeMounts": helper.as_ref().map(|status| status.active_mounts),
        "helperVersion": helper.as_ref().map(|status| status.helper_version.clone()),
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
            build_commit: Some(CANDIDATE_COMMIT.into()),
            ..Default::default()
        }
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
