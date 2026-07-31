use std::fmt;
use std::fs::OpenOptions;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing_subscriber::EnvFilter;

pub const LOCAL_SETTINGS_VERSION: u32 = 1;
pub const NORMAL_FILTER: &str = "warn,televy_backup_core=info,televybackup=info,televybackupd=info";
pub const VERBOSE_FILTER: &str =
    "info,televy_backup_core=debug,televybackup=debug,televybackupd=debug";
pub const DEBUG_FILTER: &str = "debug";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    #[default]
    Normal,
    Verbose,
    Debug,
}

impl LogLevel {
    pub fn filter(self) -> &'static str {
        match self {
            Self::Normal => NORMAL_FILTER,
            Self::Verbose => VERBOSE_FILTER,
            Self::Debug => DEBUG_FILTER,
        }
    }
}

impl fmt::Display for LogLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Normal => "normal",
            Self::Verbose => "verbose",
            Self::Debug => "debug",
        })
    }
}

impl std::str::FromStr for LogLevel {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "normal" => Ok(Self::Normal),
            "verbose" => Ok(Self::Verbose),
            "debug" => Ok(Self::Debug),
            _ => Err(format!("unsupported log level: {value}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LocalSettings {
    pub version: u32,
    pub logging: LoggingSettings,
}

impl Default for LocalSettings {
    fn default() -> Self {
        Self {
            version: LOCAL_SETTINGS_VERSION,
            logging: LoggingSettings::default(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingSettings {
    pub level: LogLevel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedLogging {
    pub configured_level: LogLevel,
    pub effective_level: String,
    pub effective_filter: String,
    pub source: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overridden_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoggingStatus {
    pub configured_level: LogLevel,
    pub effective_level: String,
    pub effective_filter: String,
    pub source: String,
    pub overridden_by: Option<String>,
    pub pending_level: Option<LogLevel>,
    pub log_directory: String,
    pub log_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub configuration_error: Option<String>,
    pub daemon_available: bool,
}

pub fn status(
    resolved: &ResolvedLogging,
    pending_level: Option<LogLevel>,
    data_dir: &Path,
    daemon_available: bool,
) -> LoggingStatus {
    let log_dir = crate::run_log::resolve_log_dir(data_dir);
    let log_bytes = directory_bytes(&log_dir).ok();
    status_with_log_bytes(
        resolved,
        pending_level,
        data_dir,
        daemon_available,
        log_bytes,
    )
}

pub fn status_with_log_bytes(
    resolved: &ResolvedLogging,
    pending_level: Option<LogLevel>,
    data_dir: &Path,
    daemon_available: bool,
    log_bytes: Option<u64>,
) -> LoggingStatus {
    let log_dir = crate::run_log::resolve_log_dir(data_dir);
    LoggingStatus {
        configured_level: resolved.configured_level,
        effective_level: resolved.effective_level.clone(),
        effective_filter: resolved.effective_filter.clone(),
        source: resolved.source.clone(),
        overridden_by: resolved.overridden_by.clone(),
        pending_level,
        log_directory: log_dir.display().to_string(),
        log_bytes,
        configuration_error: resolved.configuration_error.clone(),
        daemon_available,
    }
}

pub fn directory_bytes(path: &Path) -> io::Result<u64> {
    if !path.exists() {
        return Ok(0);
    }
    let mut total = 0_u64;
    for entry in walkdir::WalkDir::new(path) {
        let entry = entry.map_err(io::Error::other)?;
        if entry.file_type().is_file() {
            total = total.saturating_add(entry.metadata().map_err(io::Error::other)?.len());
        }
    }
    Ok(total)
}

pub fn local_settings_path(config_dir: &Path) -> PathBuf {
    config_dir.join("local.toml")
}

pub fn load(config_dir: &Path) -> io::Result<LocalSettings> {
    let path = local_settings_path(config_dir);
    let text = std::fs::read_to_string(&path)?;
    let settings: LocalSettings = toml::from_str(&text).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {}: {error}", path.display()),
        )
    })?;
    if settings.version != LOCAL_SETTINGS_VERSION {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported local settings version: {}", settings.version),
        ));
    }
    Ok(settings)
}

pub fn load_or_default(config_dir: &Path) -> (LocalSettings, Option<String>) {
    match load(config_dir) {
        Ok(settings) => (settings, None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => (LocalSettings::default(), None),
        Err(error) => (LocalSettings::default(), Some(error.to_string())),
    }
}

pub fn save(config_dir: &Path, settings: &LocalSettings) -> io::Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let path = local_settings_path(config_dir);
    let tmp = config_dir.join(format!(".local.toml.tmp-{}", uuid::Uuid::new_v4()));
    let text = toml::to_string_pretty(settings)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(&tmp)?;
    let result = (|| {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, &path)?;
        if let Ok(directory) = std::fs::File::open(config_dir) {
            let _ = directory.sync_all();
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

pub fn resolve(config_dir: &Path) -> ResolvedLogging {
    resolve_from(
        config_dir,
        std::env::var("TELEVYBACKUP_LOG").ok().as_deref(),
        std::env::var("RUST_LOG").ok().as_deref(),
    )
}

pub fn resolve_from(
    config_dir: &Path,
    televybackup_log: Option<&str>,
    rust_log: Option<&str>,
) -> ResolvedLogging {
    let (settings, configuration_error) = load_or_default(config_dir);
    let configured_level = settings.logging.level;

    if let Some(value) = televybackup_log {
        return resolve_override(
            configured_level,
            value,
            "environment",
            "TELEVYBACKUP_LOG",
            configuration_error,
        );
    }
    if let Some(value) = rust_log {
        return resolve_override(
            configured_level,
            value,
            "environment",
            "RUST_LOG",
            configuration_error,
        );
    }

    ResolvedLogging {
        configured_level,
        effective_level: configured_level.to_string(),
        effective_filter: configured_level.filter().to_owned(),
        source: if local_settings_path(config_dir).exists() {
            "local.toml".to_owned()
        } else {
            "default".to_owned()
        },
        overridden_by: None,
        configuration_error,
    }
}

fn resolve_override(
    configured_level: LogLevel,
    value: &str,
    source: &str,
    variable: &str,
    mut configuration_error: Option<String>,
) -> ResolvedLogging {
    if EnvFilter::try_new(value).is_err() {
        configuration_error = Some(format!(
            "invalid {variable} filter; using the safe Normal preset"
        ));
        return ResolvedLogging {
            configured_level,
            effective_level: LogLevel::Normal.to_string(),
            effective_filter: NORMAL_FILTER.to_owned(),
            source: source.to_owned(),
            overridden_by: Some(variable.to_owned()),
            configuration_error,
        };
    }

    let effective_level = match value {
        NORMAL_FILTER => "normal",
        VERBOSE_FILTER => "verbose",
        DEBUG_FILTER => "debug",
        _ => "custom",
    };
    ResolvedLogging {
        configured_level,
        effective_level: effective_level.to_owned(),
        effective_filter: value.to_owned(),
        source: source.to_owned(),
        overridden_by: Some(variable.to_owned()),
        configuration_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_invalid_settings_fall_back_to_normal() {
        let temp = tempfile::tempdir().unwrap();
        assert_eq!(
            resolve_from(temp.path(), None, None).configured_level,
            LogLevel::Normal
        );

        std::fs::write(
            local_settings_path(temp.path()),
            "version = 1\n[logging]\nlevel = 'trace'\n",
        )
        .unwrap();
        let resolved = resolve_from(temp.path(), None, None);
        assert_eq!(resolved.effective_level, "normal");
        assert!(resolved.configuration_error.is_some());

        std::fs::write(
            local_settings_path(temp.path()),
            "[logging]\nlevel = 'debug'\n",
        )
        .unwrap();
        let unversioned = resolve_from(temp.path(), None, None);
        assert_eq!(unversioned.effective_level, "normal");
        assert!(unversioned.configuration_error.is_some());
    }

    #[test]
    fn save_is_atomic_and_round_trips_each_level() {
        let temp = tempfile::tempdir().unwrap();
        for level in [LogLevel::Normal, LogLevel::Verbose, LogLevel::Debug] {
            let settings = LocalSettings {
                version: 1,
                logging: LoggingSettings { level },
            };
            save(temp.path(), &settings).unwrap();
            assert_eq!(load(temp.path()).unwrap(), settings);
        }
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn environment_precedence_and_invalid_filter_are_safe() {
        let temp = tempfile::tempdir().unwrap();
        save(
            temp.path(),
            &LocalSettings {
                version: 1,
                logging: LoggingSettings {
                    level: LogLevel::Debug,
                },
            },
        )
        .unwrap();

        let resolved = resolve_from(temp.path(), Some("info"), Some("debug"));
        assert_eq!(resolved.effective_filter, "info");
        assert_eq!(resolved.overridden_by.as_deref(), Some("TELEVYBACKUP_LOG"));

        let invalid = resolve_from(temp.path(), Some("[invalid"), Some("debug"));
        assert_eq!(invalid.effective_level, "normal");
        assert_eq!(invalid.effective_filter, NORMAL_FILTER);
    }
}
