use std::fs::{self, OpenOptions};
use std::io::{BufRead, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use televybackup_snapshot_helper::{
    DEFAULT_SOCKET_PATH, Method, PROTOCOL_VERSION, Request, Response, ResponseResult, StatusResult,
};
use uuid::Uuid;

use super::CliError;

pub const HELPER_LABEL: &str = "com.ivan.televybackup.snapshot-helper";
const HELPER_INSTALL_PATH: &str =
    "/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-helper";
const HELPER_PLIST_PATH: &str =
    "/Library/LaunchDaemons/com.ivan.televybackup.snapshot-helper.plist";

fn current_executable_sibling() -> Result<PathBuf, CliError> {
    let executable = std::env::current_exe().map_err(|error| {
        CliError::new("snapshot_helper.executable_unavailable", error.to_string())
    })?;
    let parent = executable.parent().ok_or_else(|| {
        CliError::new(
            "snapshot_helper.executable_unavailable",
            "CLI has no parent directory",
        )
    })?;
    let helper = parent.join("televybackup-snapshot-helper");
    if !helper.is_file() {
        return Err(CliError::new(
            "snapshot_helper.executable_unavailable",
            format!("snapshot helper binary not found: {}", helper.display()),
        ));
    }
    Ok(helper)
}

fn require_root() -> Result<(), CliError> {
    if unsafe { libc::geteuid() } != 0 {
        return Err(CliError::new(
            "snapshot_helper.admin_required",
            "snapshot helper installation must be run as root",
        ));
    }
    Ok(())
}

fn launchctl(args: &[&str]) -> Result<(), CliError> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|error| {
            CliError::retryable("snapshot_helper.launchctl_failed", error.to_string())
        })?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_string();
    Err(CliError::retryable(
        "snapshot_helper.launchctl_failed",
        if message.is_empty() {
            format!("launchctl {} failed", args.join(" "))
        } else {
            message
        },
    ))
}

fn helper_status() -> Result<StatusResult, CliError> {
    let mut stream = UnixStream::connect(DEFAULT_SOCKET_PATH).map_err(|error| {
        CliError::retryable(
            "snapshot_helper.unavailable",
            format!("connect helper: {error}"),
        )
    })?;
    let request = Request {
        version: PROTOCOL_VERSION,
        request_id: Uuid::new_v4().to_string(),
        method: Method::Status,
    };
    let mut encoded = serde_json::to_vec(&request)
        .map_err(|error| CliError::new("snapshot_helper.protocol", error.to_string()))?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .map_err(|error| CliError::retryable("snapshot_helper.unavailable", error.to_string()))?;
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(3)))
        .ok();
    let mut response_line = String::new();
    std::io::BufReader::new(stream)
        .read_line(&mut response_line)
        .map_err(|error| CliError::retryable("snapshot_helper.unavailable", error.to_string()))?;
    let response: Response = serde_json::from_str(response_line.trim())
        .map_err(|error| CliError::new("snapshot_helper.protocol", error.to_string()))?;
    if !response.ok {
        return Err(CliError::new(
            "snapshot_helper.rejected",
            response
                .message
                .unwrap_or_else(|| "helper rejected status request".into()),
        ));
    }
    match response.result {
        Some(ResponseResult::Status(status)) => Ok(status),
        _ => Err(CliError::new(
            "snapshot_helper.protocol",
            "status response was missing",
        )),
    }
}

fn reject_if_busy(operation: &str) -> Result<(), CliError> {
    match helper_status() {
        Ok(status) if status.active_leases > 0 || status.pending_cleanup > 0 => Err(
            CliError::new(
                "snapshot_helper.busy",
                format!(
                    "cannot {operation} while helper has {} active lease(s) and {} pending cleanup item(s)",
                    status.active_leases, status.pending_cleanup
                ),
            )
            .with_details(json!({
                "activeLeases": status.active_leases,
                "pendingCleanup": status.pending_cleanup,
            })),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.code == "snapshot_helper.unavailable" => Ok(()),
        Err(error) => Err(error),
    }
}

fn plist_contents() -> &'static str {
    r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>com.ivan.televybackup.snapshot-helper</string>
  <key>ProgramArguments</key><array><string>/Library/PrivilegedHelperTools/com.ivan.televybackup.snapshot-helper</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>/var/log/com.ivan.televybackup.snapshot-helper.log</string>
  <key>StandardErrorPath</key><string>/var/log/com.ivan.televybackup.snapshot-helper.log</string>
</dict></plist>
"#
}

fn atomic_copy(source: &Path, destination: &Path) -> Result<(), CliError> {
    let parent = destination.parent().ok_or_else(|| {
        CliError::new(
            "snapshot_helper.install_failed",
            "helper destination has no parent",
        )
    })?;
    fs::create_dir_all(parent)
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))?;
    let staged = destination.with_extension(format!("tmp-{}", std::process::id()));
    fs::copy(source, &staged)
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o755))
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))?;
    fs::rename(&staged, destination)
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::new("snapshot_helper.install_failed", "plist has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))?;
    let staged = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&staged)
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))?;
    fs::set_permissions(&staged, fs::Permissions::from_mode(0o644))
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))?;
    fs::rename(staged, path)
        .map_err(|error| CliError::new("snapshot_helper.install_failed", error.to_string()))
}

pub fn install(json_output: bool) -> Result<(), CliError> {
    require_root()?;
    reject_if_busy("install or update the snapshot helper")?;
    let source = current_executable_sibling()?;
    atomic_copy(&source, Path::new(HELPER_INSTALL_PATH))?;
    atomic_write(Path::new(HELPER_PLIST_PATH), plist_contents().as_bytes())?;
    let _ = launchctl(&["bootout", "system", HELPER_LABEL]);
    launchctl(&["bootstrap", "system", HELPER_PLIST_PATH])?;
    if json_output {
        println!(
            "{}",
            json!({"installed": true, "label": HELPER_LABEL, "binary": HELPER_INSTALL_PATH, "plist": HELPER_PLIST_PATH})
        );
    } else {
        println!("snapshot helper installed");
    }
    Ok(())
}

pub fn uninstall(json_output: bool) -> Result<(), CliError> {
    require_root()?;
    reject_if_busy("uninstall the snapshot helper")?;
    let _ = launchctl(&["bootout", "system", HELPER_LABEL]);
    if Path::new(HELPER_PLIST_PATH).exists() {
        fs::remove_file(HELPER_PLIST_PATH).map_err(|error| {
            CliError::new("snapshot_helper.uninstall_failed", error.to_string())
        })?;
    }
    if Path::new(HELPER_INSTALL_PATH).exists() {
        fs::remove_file(HELPER_INSTALL_PATH).map_err(|error| {
            CliError::new("snapshot_helper.uninstall_failed", error.to_string())
        })?;
    }
    if json_output {
        println!(
            "{}",
            json!({"uninstalled": true, "label": HELPER_LABEL, "journalPreserved": true})
        );
    } else {
        println!("snapshot helper uninstalled; journal preserved");
    }
    Ok(())
}

pub fn status(json_output: bool) -> Result<(), CliError> {
    let loaded = Command::new("/bin/launchctl")
        .args(["print", &format!("system/{HELPER_LABEL}")])
        .output()
        .is_ok_and(|output| output.status.success());
    let helper = helper_status().ok();
    let payload = json!({
        "installed": Path::new(HELPER_INSTALL_PATH).is_file() && Path::new(HELPER_PLIST_PATH).is_file(),
        "launchdLoaded": loaded,
        "label": HELPER_LABEL,
        "activeLeases": helper.as_ref().map(|status| status.active_leases),
        "pendingCleanup": helper.as_ref().map(|status| status.pending_cleanup),
        "helperVersion": helper.as_ref().map(|status| status.helper_version.clone()),
    });
    if json_output {
        println!("{payload}");
    } else {
        println!(
            "snapshot helper: {}",
            if loaded { "loaded" } else { "not loaded" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_uses_fixed_root_owned_paths() {
        let plist = plist_contents();
        assert!(plist.contains(HELPER_LABEL));
        assert!(plist.contains(HELPER_INSTALL_PATH));
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
    }
}
