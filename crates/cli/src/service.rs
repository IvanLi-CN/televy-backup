use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::CliError;

pub const SERVICE_LABEL: &str = "com.ivan.televybackup.daemon";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ServiceManifest {
    schema_version: u32,
    label: String,
    version: String,
    config_dir: String,
    data_dir: String,
    daemon_sha256: String,
    helper_sha256: String,
}

fn service_root(config_dir: &Path) -> PathBuf {
    std::env::var_os("TELEVYBACKUP_SERVICE_ROOT")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir.join("service"))
}

fn manifest_path(config_dir: &Path) -> PathBuf {
    service_root(config_dir).join("installation.json")
}

fn plist_path() -> PathBuf {
    if let Some(path) = std::env::var_os("TELEVYBACKUP_LAUNCHAGENT_PLIST") {
        return PathBuf::from(path);
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join("Library/LaunchAgents")
        .join(format!("{SERVICE_LABEL}.plist"))
}

fn launchctl_path() -> PathBuf {
    std::env::var_os("TELEVYBACKUP_LAUNCHCTL")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/bin/launchctl"))
}

fn current_version() -> String {
    option_env!("TELEVYBACKUP_BUILD_VERSION")
        .unwrap_or(env!("CARGO_PKG_VERSION"))
        .to_string()
}

fn sibling_binary(name: &str) -> Result<PathBuf, CliError> {
    let exe = std::env::current_exe().map_err(|e| {
        CliError::new(
            "service.executable_unavailable",
            format!("current executable: {e}"),
        )
    })?;
    let parent = exe.parent().ok_or_else(|| {
        CliError::new(
            "service.executable_unavailable",
            "current executable has no parent",
        )
    })?;
    let path = parent.join(name);
    if !path.is_file() {
        return Err(CliError::new(
            "service.executable_unavailable",
            format!("managed service binary not found: {}", path.display()),
        ));
    }
    Ok(path)
}

fn sha256_file(path: &Path) -> Result<String, CliError> {
    let bytes = fs::read(path).map_err(|e| {
        CliError::new(
            "service.executable_unavailable",
            format!("read {}: {e}", path.display()),
        )
    })?;
    Ok(format!("{:x}", Sha256::digest(bytes)))
}

fn read_manifest(config_dir: &Path) -> Result<Option<ServiceManifest>, CliError> {
    let path = manifest_path(config_dir);
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(&path).map_err(|e| {
        CliError::new(
            "service.state_invalid",
            format!("read {}: {e}", path.display()),
        )
    })?;
    serde_json::from_slice(&bytes).map(Some).map_err(|e| {
        CliError::new(
            "service.state_invalid",
            format!("parse {}: {e}", path.display()),
        )
    })
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), CliError> {
    let parent = path.parent().ok_or_else(|| {
        CliError::new(
            "service.state_invalid",
            format!("path has no parent: {}", path.display()),
        )
    })?;
    fs::create_dir_all(parent).map_err(|e| {
        CliError::new(
            "service.state_invalid",
            format!("create {}: {e}", parent.display()),
        )
    })?;
    let temp = path.with_extension("tmp");
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(&temp)
        .map_err(|e| {
            CliError::new(
                "service.state_invalid",
                format!("write {}: {e}", temp.display()),
            )
        })?;
    file.write_all(contents)
        .and_then(|_| file.sync_all())
        .map_err(|e| {
            CliError::new(
                "service.state_invalid",
                format!("flush {}: {e}", temp.display()),
            )
        })?;
    fs::rename(&temp, path).map_err(|e| {
        CliError::new(
            "service.state_invalid",
            format!("activate {}: {e}", path.display()),
        )
    })
}

fn launchctl(args: &[&str]) -> Result<(), CliError> {
    let output = Command::new(launchctl_path())
        .args(args)
        .output()
        .map_err(|e| CliError::retryable("service.launchctl_failed", format!("launchctl: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(CliError::retryable(
            "service.launchctl_failed",
            if stderr.is_empty() {
                format!("launchctl {} failed", args.join(" "))
            } else {
                stderr
            },
        ))
    }
}

fn gui_domain() -> String {
    let uid = unsafe { libc::geteuid() };
    format!("gui/{uid}")
}

pub fn managed_service_matches(config_dir: &Path, data_dir: &Path) -> bool {
    read_manifest(config_dir)
        .ok()
        .flatten()
        .is_some_and(|manifest| {
            manifest.label == SERVICE_LABEL
                && manifest.config_dir == config_dir.to_string_lossy()
                && manifest.data_dir == data_dir.to_string_lossy()
        })
}

pub fn kickstart_service() -> Result<(), CliError> {
    launchctl(&[
        "kickstart",
        "-k",
        &format!("{}/{}", gui_domain(), SERVICE_LABEL),
    ])
}

pub fn stop_service() -> Result<(), CliError> {
    let domain = gui_domain();
    let _ = launchctl(&["disable", &format!("{}/{}", domain, SERVICE_LABEL)]);
    launchctl(&["bootout", &domain, SERVICE_LABEL])
}

fn plist_contents(manifest: &ServiceManifest, daemon_path: &Path) -> String {
    let log_dir = PathBuf::from(&manifest.data_dir).join("logs");
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>Label</key><string>{}</string>
  <key>ProgramArguments</key><array><string>{}</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>TELEVYBACKUP_CONFIG_DIR</key><string>{}</string>
    <key>TELEVYBACKUP_DATA_DIR</key><string>{}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>{}/televybackupd.stdout.log</string>
  <key>StandardErrorPath</key><string>{}/televybackupd.stderr.log</string>
</dict></plist>
"#,
        manifest.label,
        daemon_path.display(),
        xml_escape(&manifest.config_dir),
        xml_escape(&manifest.data_dir),
        log_dir.display(),
        log_dir.display(),
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn install_inner(
    config_dir: &Path,
    data_dir: &Path,
    replace: bool,
) -> Result<ServiceManifest, CliError> {
    let root = service_root(config_dir);
    let versions = root.join("versions");
    fs::create_dir_all(&versions).map_err(|e| {
        CliError::new(
            "service.install_failed",
            format!("create {}: {e}", versions.display()),
        )
    })?;
    // launchd opens these paths before starting the daemon, so create the
    // parent directory during installation instead of relying on daemon code.
    fs::create_dir_all(data_dir.join("logs")).map_err(|e| {
        CliError::new(
            "service.install_failed",
            format!("create {}: {e}", data_dir.join("logs").display()),
        )
    })?;
    let daemon = sibling_binary("televybackupd")?;
    let helper = sibling_binary("televybackup-mtproto-helper")?;
    let version = current_version();
    let new_manifest = ServiceManifest {
        schema_version: 1,
        label: SERVICE_LABEL.to_string(),
        version: version.clone(),
        config_dir: config_dir.to_string_lossy().into_owned(),
        data_dir: data_dir.to_string_lossy().into_owned(),
        daemon_sha256: sha256_file(&daemon)?,
        helper_sha256: sha256_file(&helper)?,
    };

    if let Some(old) = read_manifest(config_dir)? {
        let same_environment =
            old.config_dir == new_manifest.config_dir && old.data_dir == new_manifest.data_dir;
        if !same_environment && !replace {
            return Err(CliError::new(
                "service.environment_conflict",
                format!(
                    "managed service is bound to config={} data={}; pass --replace to change it",
                    old.config_dir, old.data_dir
                ),
            )
            .with_details(serde_json::json!({"existing": old, "requested": new_manifest})));
        }
        if same_environment
            && old.version == new_manifest.version
            && old.daemon_sha256 == new_manifest.daemon_sha256
            && old.helper_sha256 == new_manifest.helper_sha256
            && plist_path().is_file()
        {
            return Ok(old);
        }
    } else if plist_path().is_file() && !replace {
        return Err(CliError::new(
            "service.ownership_conflict",
            format!(
                "LaunchAgent plist exists without product ownership: {}; pass --replace to adopt it",
                plist_path().display()
            ),
        ));
    }

    let stage = versions.join(format!(".staging-{}-{}", std::process::id(), version));
    if stage.exists() {
        fs::remove_dir_all(&stage)
            .map_err(|e| CliError::new("service.install_failed", e.to_string()))?;
    }
    fs::create_dir_all(&stage)
        .map_err(|e| CliError::new("service.install_failed", e.to_string()))?;
    fs::copy(&daemon, stage.join("televybackupd"))
        .map_err(|e| CliError::new("service.install_failed", e.to_string()))?;
    fs::copy(&helper, stage.join("televybackup-mtproto-helper"))
        .map_err(|e| CliError::new("service.install_failed", e.to_string()))?;
    let active_version_dir = versions.join(&version);
    let rollback_dir = versions.join(format!(".rollback-{}-{}", std::process::id(), version));
    if rollback_dir.exists() {
        fs::remove_dir_all(&rollback_dir)
            .map_err(|e| CliError::new("service.install_failed", e.to_string()))?;
    }
    if active_version_dir.exists() {
        fs::rename(&active_version_dir, &rollback_dir)
            .map_err(|e| CliError::new("service.install_failed", e.to_string()))?;
    }
    fs::rename(&stage, &active_version_dir)
        .map_err(|e| CliError::new("service.install_failed", e.to_string()))?;
    for entry in fs::read_dir(&active_version_dir)
        .into_iter()
        .flatten()
        .flatten()
    {
        let mut perms = entry
            .metadata()
            .map_err(|e| CliError::new("service.install_failed", e.to_string()))?
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        fs::set_permissions(entry.path(), perms)
            .map_err(|e| CliError::new("service.install_failed", e.to_string()))?;
    }

    let plist = plist_path();
    let old_plist = if plist.is_file() {
        Some(fs::read(&plist).map_err(|e| CliError::new("service.install_failed", e.to_string()))?)
    } else {
        None
    };
    let plist_text = plist_contents(&new_manifest, &active_version_dir.join("televybackupd"));
    atomic_write(&plist, plist_text.as_bytes())?;
    let domain = gui_domain();
    let _ = launchctl(&["bootout", &domain, SERVICE_LABEL]);
    if let Err(error) = launchctl(&["bootstrap", &domain, &plist.to_string_lossy()]) {
        if let Some(old_bytes) = old_plist {
            let _ = atomic_write(&plist, &old_bytes);
            let _ = launchctl(&["bootstrap", &domain, &plist.to_string_lossy()]);
        } else {
            let _ = fs::remove_file(&plist);
        }
        let _ = fs::remove_dir_all(&active_version_dir);
        if rollback_dir.exists() {
            let _ = fs::rename(&rollback_dir, &active_version_dir);
        }
        return Err(error.with_details(serde_json::json!({"rolledBack": true})));
    }
    atomic_write(
        &manifest_path(config_dir),
        serde_json::to_vec_pretty(&new_manifest).unwrap().as_slice(),
    )?;
    if rollback_dir.exists() {
        let _ = fs::remove_dir_all(&rollback_dir);
    }

    let mut retained = fs::read_dir(&versions)
        .map_err(|e| CliError::new("service.install_failed", e.to_string()))?
        .flatten()
        .filter(|entry| {
            entry.path().is_dir() && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    retained.sort();
    while retained.len() > 2 {
        let old = retained.remove(0);
        if old != active_version_dir {
            let _ = fs::remove_dir_all(old);
        }
    }
    Ok(new_manifest)
}

pub fn install_service(
    config_dir: &Path,
    data_dir: &Path,
    replace: bool,
    json: bool,
) -> Result<(), CliError> {
    let manifest = install_inner(config_dir, data_dir, replace)?;
    if json {
        println!(
            "{}",
            serde_json::json!({"installed": true, "label": SERVICE_LABEL, "version": manifest.version, "configDir": manifest.config_dir, "dataDir": manifest.data_dir, "replaced": replace})
        );
    } else {
        println!("managed service installed ({})", manifest.version);
    }
    Ok(())
}

pub fn uninstall_service(config_dir: &Path, json: bool) -> Result<(), CliError> {
    let plist = plist_path();
    let domain = gui_domain();
    if plist.is_file() {
        let _ = launchctl(&["bootout", &domain, SERVICE_LABEL]);
        fs::remove_file(&plist)
            .map_err(|e| CliError::new("service.uninstall_failed", e.to_string()))?;
    }
    let root = service_root(config_dir);
    if root.is_dir() {
        fs::remove_dir_all(&root)
            .map_err(|e| CliError::new("service.uninstall_failed", e.to_string()))?;
    }
    if json {
        println!(
            "{}",
            serde_json::json!({"uninstalled": true, "label": SERVICE_LABEL, "preserved": ["config", "data", "logs", "keychain"]})
        );
    } else {
        println!("managed service uninstalled; user data preserved");
    }
    Ok(())
}

pub fn service_status(config_dir: &Path, data_dir: &Path, json: bool) -> Result<(), CliError> {
    let manifest = read_manifest(config_dir)?;
    let launchd_loaded =
        launchctl(&["print", &format!("{}/{}", gui_domain(), SERVICE_LABEL)]).is_ok();
    let environment_match = manifest.as_ref().map(|m| {
        m.config_dir == config_dir.to_string_lossy() && m.data_dir == data_dir.to_string_lossy()
    });
    let payload = serde_json::json!({
        "installed": manifest.is_some(),
        "label": SERVICE_LABEL,
        "launchdLoaded": launchd_loaded,
        "environmentMatch": environment_match,
        "version": manifest.as_ref().map(|m| m.version.clone()),
        "configDir": manifest.as_ref().map(|m| m.config_dir.clone()),
        "dataDir": manifest.as_ref().map(|m| m.data_dir.clone()),
        "plist": plist_path(),
    });
    if json {
        println!("{payload}");
    } else if let Some(manifest) = manifest {
        println!(
            "managed service {}: {}",
            manifest.version,
            if launchd_loaded {
                "loaded"
            } else {
                "not loaded"
            }
        );
    } else {
        println!("managed service: not installed");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plist_escapes_environment_paths() {
        let manifest = ServiceManifest {
            schema_version: 1,
            label: SERVICE_LABEL.into(),
            version: "1.2.3".into(),
            config_dir: "/tmp/a&b".into(),
            data_dir: "/tmp/c<d".into(),
            daemon_sha256: "a".into(),
            helper_sha256: "b".into(),
        };
        let plist = plist_contents(&manifest, Path::new("/tmp/daemon"));
        assert!(plist.contains("/tmp/a&amp;b"));
        assert!(plist.contains("/tmp/c&lt;d"));
        assert!(plist.contains("KeepAlive"));
    }
}
