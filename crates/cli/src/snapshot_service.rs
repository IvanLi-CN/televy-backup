use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use televybackup_snapshot_access::{
    Method, PROTOCOL_VERSION, Request, Response, ResponseResult, StatusResult,
};
use uuid::Uuid;

use super::CliError;

pub const ACCESS_LABEL: &str = "com.ivan.televybackup.snapshot-access";
const MANIFEST_FILE: &str = "snapshot-access/service.json";

fn user_service_target(domain: &str) -> String {
    format!("{domain}/{ACCESS_LABEL}")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}
fn plist_path() -> PathBuf {
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

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), CliError> {
    let parent = path
        .parent()
        .ok_or_else(|| CliError::new("snapshot_access.install_failed", "path has no parent"))?;
    fs::create_dir_all(parent)
        .map_err(|error| CliError::new("snapshot_access.install_failed", error.to_string()))?;
    let temp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|error| CliError::new("snapshot_access.install_failed", error.to_string()))?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|error| CliError::new("snapshot_access.install_failed", error.to_string()))?;
    fs::rename(temp, path)
        .map_err(|error| CliError::new("snapshot_access.install_failed", error.to_string()))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
fn plist_contents(app: &Path, config_dir: &Path, data_dir: &Path) -> String {
    let socket = data_dir.join("snapshot-access/access.sock");
    let journal = data_dir.join("snapshot-access/journal.sqlite");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict><key>Label</key><string>{ACCESS_LABEL}</string>
<key>ProgramArguments</key><array><string>{}</string></array><key>EnvironmentVariables</key><dict>
<key>TELEVYBACKUP_CONFIG_DIR</key><string>{}</string><key>TELEVYBACKUP_DATA_DIR</key><string>{}</string>
<key>TELEVYBACKUP_SNAPSHOT_SOCKET</key><string>{}</string><key>TELEVYBACKUP_SNAPSHOT_JOURNAL</key><string>{}</string>
</dict><key>RunAtLoad</key><true/><key>KeepAlive</key><true/>
</dict></plist>
"#,
        xml_escape(&app.to_string_lossy()),
        xml_escape(&config_dir.to_string_lossy()),
        xml_escape(&data_dir.to_string_lossy()),
        xml_escape(&socket.to_string_lossy()),
        xml_escape(&journal.to_string_lossy())
    )
}

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
    // Status includes a bounded round trip to the mount helper. The Access App
    // uses its SQLite journal on that path, so three seconds can falsely report
    // a healthy service as unavailable on a busy machine.
    stream
        .set_read_timeout(Some(std::time::Duration::from_secs(12)))
        .ok();
    let mut line = String::new();
    std::io::BufReader::new(stream)
        .read_line(&mut line)
        .map_err(|error| CliError::retryable("snapshot_access.unavailable", error.to_string()))?;
    let response: Response = serde_json::from_str(line.trim())
        .map_err(|error| CliError::new("snapshot_access.protocol", error.to_string()))?;
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

pub fn install(
    app: PathBuf,
    config_dir: &Path,
    data_dir: &Path,
    json_output: bool,
) -> Result<(), CliError> {
    let app = app
        .canonicalize()
        .map_err(|error| CliError::new("snapshot_access.app_invalid", error.to_string()))?;
    let executable = if app.extension().is_some_and(|extension| extension == "app") {
        app.join("Contents/MacOS/televybackup-snapshot-access")
    } else {
        app.clone()
    };
    if !executable.is_file() {
        return Err(CliError::new(
            "snapshot_access.app_invalid",
            format!(
                "Snapshot Access executable not found: {}",
                executable.display()
            ),
        ));
    }
    let plist = plist_path();
    atomic_write(
        &plist,
        plist_contents(&executable, config_dir, data_dir).as_bytes(),
    )?;
    let domain = format!("gui/{}", unsafe { libc::geteuid() });
    let service = user_service_target(&domain);
    let _ = launchctl(&["bootout", &service]);
    launchctl(&["bootstrap", &domain, plist.to_string_lossy().as_ref()])?;
    let manifest = json!({"schemaVersion": 1, "label": ACCESS_LABEL, "appPath": app, "executablePath": executable, "plistPath": plist, "configDir": config_dir, "dataDir": data_dir});
    atomic_write(
        &manifest_path(config_dir),
        serde_json::to_string_pretty(&manifest).unwrap().as_bytes(),
    )?;
    if json_output {
        println!(
            "{}",
            json!({"installed": true, "label": ACCESS_LABEL, "appPath": app, "plistPath": plist})
        );
    } else {
        println!("snapshot access installed for {}", app.display());
    }
    Ok(())
}

pub fn uninstall(config_dir: &Path, data_dir: &Path, json_output: bool) -> Result<(), CliError> {
    if let Ok(payload) = status(config_dir, data_dir, true)
        && payload["activeLeases"].as_u64().unwrap_or(0) > 0
    {
        return Err(CliError::new(
            "snapshot_access.busy",
            "cannot uninstall while a snapshot lease is active",
        ));
    }
    let domain = format!("gui/{}", unsafe { libc::geteuid() });
    let service = user_service_target(&domain);
    let _ = launchctl(&["bootout", &service]);
    let plist = plist_path();
    if plist.exists() {
        fs::remove_file(plist).map_err(|error| {
            CliError::new("snapshot_access.uninstall_failed", error.to_string())
        })?;
    }
    if json_output {
        println!(
            "{}",
            json!({"uninstalled": true, "label": ACCESS_LABEL, "journalPreserved": true})
        );
    } else {
        println!("snapshot access uninstalled; journal preserved");
    }
    Ok(())
}

pub fn status(
    config_dir: &Path,
    data_dir: &Path,
    json_output: bool,
) -> Result<serde_json::Value, CliError> {
    let manifest = fs::read(manifest_path(config_dir))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
    let socket = manifest
        .as_ref()
        .and_then(|value| value.get("dataDir"))
        .and_then(serde_json::Value::as_str)
        .map(|dir| PathBuf::from(dir).join("snapshot-access/access.sock"))
        .unwrap_or_else(|| data_dir.join("snapshot-access/access.sock"));
    let helper = status_from_socket(&socket).ok();
    let payload = json!({"installed": manifest.is_some() && plist_path().is_file(), "label": ACCESS_LABEL, "appPath": manifest.as_ref().and_then(|value| value.get("appPath")), "executablePath": manifest.as_ref().and_then(|value| value.get("executablePath")), "plistPath": plist_path(), "serviceReachable": helper.is_some(), "activeLeases": helper.as_ref().map(|value| value.active_leases).unwrap_or(0), "pendingCleanup": helper.as_ref().map(|value| value.pending_cleanup).unwrap_or(0), "accessAppVersion": helper.as_ref().map(|value| value.access_app_version.clone()), "fdaReady": helper.as_ref().map(|value| value.fda_ready).unwrap_or(false), "mountHelperReachable": helper.as_ref().map(|value| value.mount_helper_reachable).unwrap_or(false), "mountHelperVersion": helper.as_ref().and_then(|value| value.mount_helper_version.clone()), "mountHelperError": helper.as_ref().and_then(|value| value.mount_helper_error.clone())});
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_agent_plist_points_at_user_access_app() {
        let plist = plist_contents(
            Path::new(
                "/Users/test/Applications/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access",
            ),
            Path::new("/Users/test/Library/Application Support/TelevyBackup"),
            Path::new("/Users/test/Library/Application Support/TelevyBackup"),
        );
        assert!(plist.contains(ACCESS_LABEL));
        assert!(plist.contains("RunAtLoad"));
        assert!(plist.contains("TELEVYBACKUP_SNAPSHOT_SOCKET"));
        assert!(!plist.contains("LaunchDaemons"));
        assert!(!plist.contains("PrivilegedHelper"));
        assert!(!plist.contains("administrator privileges"));
    }

    #[test]
    fn plist_escapes_paths_as_xml() {
        let plist = plist_contents(
            Path::new("/Users/test/A&B.app/Contents/MacOS/access"),
            Path::new("/Users/test/config<one>"),
            Path::new("/Users/test/data"),
        );
        assert!(plist.contains("A&amp;B.app"));
        assert!(plist.contains("config&lt;one&gt;"));
    }

    #[test]
    fn launchctl_uses_a_fully_qualified_user_service_target() {
        assert_eq!(
            user_service_target("gui/501"),
            "gui/501/com.ivan.televybackup.snapshot-access"
        );
    }
}
