use std::ffi::CString;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use televybackup_snapshot_access::mount_helper;

use super::CliError;

pub const MOUNT_HELPER_LABEL: &str = "com.ivan.televybackup.snapshot-mount-helper";
const MOUNT_HELPER_INSTALL_PATH: &str =
    "/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-mount-helper";
const MOUNT_HELPER_PLIST_PATH: &str =
    "/Library/LaunchDaemons/com.ivan.televybackup.snapshot-mount-helper.plist";

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

pub fn install(json_output: bool) -> Result<(), CliError> {
    require_root()?;
    reject_if_busy("install or update the snapshot mount helper")?;
    let source = current_executable_sibling()?;
    atomic_copy(&source, Path::new(MOUNT_HELPER_INSTALL_PATH))?;
    atomic_write(
        Path::new(MOUNT_HELPER_PLIST_PATH),
        plist_contents().as_bytes(),
    )?;
    let service = system_service_target();
    let _ = launchctl(&["bootout", &service]);
    launchctl(&["bootstrap", "system", MOUNT_HELPER_PLIST_PATH])?;
    if json_output {
        println!(
            "{}",
            json!({"installed": true, "label": MOUNT_HELPER_LABEL, "binary": MOUNT_HELPER_INSTALL_PATH, "plist": MOUNT_HELPER_PLIST_PATH})
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
}
