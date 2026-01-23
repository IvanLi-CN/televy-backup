use std::path::{Path, PathBuf};

use serde::de::Error as _;
use serde::{Deserialize, Serialize};

use crate::crypto::FRAMING_OVERHEAD_BYTES;
use crate::storage::MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES;
use crate::{Error, Result};

pub const SETTINGS_SCHEMA_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SettingsV2 {
    pub version: u32,
    #[serde(default)]
    pub schedule: Schedule,
    #[serde(default)]
    pub retention: Retention,
    #[serde(default)]
    pub chunking: Chunking,
    #[serde(default)]
    pub telegram: TelegramGlobal,
    #[serde(default)]
    pub telegram_endpoints: Vec<TelegramEndpoint>,
    #[serde(default)]
    pub targets: Vec<Target>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Schedule {
    pub enabled: bool,
    pub kind: String,
    pub hourly_minute: u8,
    pub daily_at: String,
    pub timezone: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Retention {
    pub keep_last_snapshots: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunking {
    pub min_bytes: u32,
    pub avg_bytes: u32,
    pub max_bytes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramGlobal {
    pub mode: String,
    #[serde(default)]
    pub mtproto: TelegramMtprotoGlobal,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramMtprotoGlobal {
    pub api_id: i32,
    pub api_hash_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramEndpoint {
    pub id: String,
    pub mode: String,
    pub chat_id: String,
    pub bot_token_key: String,
    #[serde(default)]
    pub mtproto: TelegramEndpointMtproto,
    #[serde(default)]
    pub rate_limit: TelegramRateLimit,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramEndpointMtproto {
    pub session_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelegramRateLimit {
    pub max_concurrent_uploads: u32,
    pub min_delay_ms: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Target {
    pub id: String,
    pub source_path: String,
    #[serde(default)]
    pub label: String,
    pub endpoint_id: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub schedule: Option<TargetScheduleOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TargetScheduleOverride {
    pub enabled: Option<bool>,
    pub kind: Option<String>,
    pub hourly_minute: Option<u8>,
    pub daily_at: Option<String>,
}

fn default_true() -> bool {
    true
}

impl Default for Schedule {
    fn default() -> Self {
        Self {
            enabled: false,
            kind: "hourly".to_string(),
            hourly_minute: 0,
            daily_at: "02:00".to_string(),
            timezone: "local".to_string(),
        }
    }
}

impl Default for Retention {
    fn default() -> Self {
        Self {
            keep_last_snapshots: 7,
        }
    }
}

impl Default for Chunking {
    fn default() -> Self {
        Self {
            min_bytes: 1024 * 1024,
            avg_bytes: 4 * 1024 * 1024,
            max_bytes: 10 * 1024 * 1024,
        }
    }
}

impl Default for TelegramMtprotoGlobal {
    fn default() -> Self {
        Self {
            api_id: 0,
            api_hash_key: "telegram.mtproto.api_hash".to_string(),
        }
    }
}

impl Default for TelegramGlobal {
    fn default() -> Self {
        Self {
            mode: "mtproto".to_string(),
            mtproto: TelegramMtprotoGlobal::default(),
        }
    }
}

impl Default for TelegramRateLimit {
    fn default() -> Self {
        Self {
            max_concurrent_uploads: 2,
            min_delay_ms: 250,
        }
    }
}

impl Default for SettingsV2 {
    fn default() -> Self {
        Self {
            version: SETTINGS_SCHEMA_VERSION,
            schedule: Schedule::default(),
            retention: Retention::default(),
            chunking: Chunking::default(),
            telegram: TelegramGlobal::default(),
            telegram_endpoints: Vec::new(),
            targets: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct SettingsV1 {
    #[serde(default)]
    sources: Vec<String>,
    #[serde(default)]
    schedule: Schedule,
    #[serde(default)]
    retention: Retention,
    #[serde(default)]
    chunking: Chunking,
    telegram: TelegramV1,
}

#[derive(Debug, Clone, Deserialize)]
struct TelegramV1 {
    mode: String,
    chat_id: String,
    bot_token_key: String,
    #[serde(default)]
    mtproto: TelegramV1Mtproto,
    #[serde(default)]
    rate_limit: TelegramRateLimit,
}

#[derive(Debug, Clone, Deserialize)]
struct TelegramV1Mtproto {
    api_id: i32,
    api_hash_key: String,
    session_key: String,
}

impl Default for TelegramV1Mtproto {
    fn default() -> Self {
        Self {
            api_id: 0,
            api_hash_key: "telegram.mtproto.api_hash".to_string(),
            session_key: "telegram.mtproto.session".to_string(),
        }
    }
}

pub fn config_path(config_dir: &Path) -> PathBuf {
    config_dir.join("config.toml")
}

pub fn load_settings_v2(config_dir: &Path) -> Result<SettingsV2> {
    let path = config_path(config_dir);
    if !path.exists() {
        return Ok(SettingsV2::default());
    }

    let text = std::fs::read_to_string(&path).map_err(|e| Error::InvalidConfig {
        message: format!("config read failed: {e}"),
    })?;

    parse_settings_v2(&text).map_err(|e| Error::InvalidConfig {
        message: format!("config invalid: {e}"),
    })
}

pub fn parse_settings_v2(text: &str) -> std::result::Result<SettingsV2, toml::de::Error> {
    let raw: toml::Value = toml::from_str(text)?;
    let version = raw
        .get("version")
        .and_then(|v| v.as_integer())
        .and_then(|v| u32::try_from(v).ok());

    match version {
        Some(SETTINGS_SCHEMA_VERSION) => {
            let mut s = toml::from_str::<SettingsV2>(text)?;
            normalize_settings_v2(&mut s);
            Ok(s)
        }
        Some(other) => {
            let err = toml::de::Error::custom(format!(
                "unsupported settings schema version: {other} (expected {SETTINGS_SCHEMA_VERSION})"
            ));
            Err(err)
        }
        None => {
            let v1: SettingsV1 = toml::from_str(text)?;
            let mut s = migrate_v1_to_v2(v1);
            normalize_settings_v2(&mut s);
            Ok(s)
        }
    }
}

fn normalize_settings_v2(settings: &mut SettingsV2) {
    let multi_endpoints = settings.telegram_endpoints.len() > 1;
    for ep in &mut settings.telegram_endpoints {
        let key = ep.mtproto.session_key.trim();
        if key.is_empty() || (multi_endpoints && key == "telegram.mtproto.session") {
            ep.mtproto.session_key = endpoint_session_key_default(&ep.id);
        }
    }
}

pub fn to_toml_v2(settings: &SettingsV2) -> Result<String> {
    validate_settings_schema_v2(settings)?;
    toml::to_string(settings).map_err(|e| Error::InvalidConfig {
        message: format!("config encode failed: {e}"),
    })
}

pub fn save_settings_v2(config_dir: &Path, settings: &SettingsV2) -> Result<()> {
    validate_settings_schema_v2(settings)?;

    let path = config_path(config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| Error::InvalidConfig {
            message: format!("config dir create failed: {e}"),
        })?;
    }

    let text = toml::to_string(settings).map_err(|e| Error::InvalidConfig {
        message: format!("config encode failed: {e}"),
    })?;

    atomic_write(&path, text.as_bytes()).map_err(|e| Error::InvalidConfig {
        message: format!("config write failed: {e}"),
    })?;
    Ok(())
}

pub fn validate_settings_schema_v2(settings: &SettingsV2) -> Result<()> {
    if settings.version != SETTINGS_SCHEMA_VERSION {
        return Err(Error::InvalidConfig {
            message: format!(
                "settings.version must be {SETTINGS_SCHEMA_VERSION} (got {})",
                settings.version
            ),
        });
    }

    if settings.telegram.mode.trim() != "mtproto" {
        return Err(Error::InvalidConfig {
            message: "telegram.mode must be \"mtproto\"".to_string(),
        });
    }

    if settings.retention.keep_last_snapshots < 1 {
        return Err(Error::InvalidConfig {
            message: "retention.keep_last_snapshots must be >= 1".to_string(),
        });
    }

    if settings.chunking.min_bytes == 0
        || settings.chunking.avg_bytes == 0
        || settings.chunking.max_bytes == 0
    {
        return Err(Error::InvalidConfig {
            message: "chunk sizes must be > 0".to_string(),
        });
    }
    if !(settings.chunking.min_bytes <= settings.chunking.avg_bytes
        && settings.chunking.avg_bytes <= settings.chunking.max_bytes)
    {
        return Err(Error::InvalidConfig {
            message: "chunk sizes must satisfy min <= avg <= max".to_string(),
        });
    }

    // Validate chunk sizes against FastCDC bounds (avoid runtime panics).
    if settings.chunking.max_bytes <= fastcdc::v2020::MAXIMUM_MAX {
        let min_ok = (fastcdc::v2020::MINIMUM_MIN..=fastcdc::v2020::MINIMUM_MAX)
            .contains(&settings.chunking.min_bytes);
        let avg_ok = (fastcdc::v2020::AVERAGE_MIN..=fastcdc::v2020::AVERAGE_MAX)
            .contains(&settings.chunking.avg_bytes);
        let max_ok = (fastcdc::v2020::MAXIMUM_MIN..=fastcdc::v2020::MAXIMUM_MAX)
            .contains(&settings.chunking.max_bytes);
        if !(min_ok && avg_ok && max_ok) {
            return Err(Error::InvalidConfig {
                message: format!(
                    "chunk sizes out of bounds for fastcdc::v2020 (min={}..={}, avg={}..={}, max>={})",
                    fastcdc::v2020::MINIMUM_MIN,
                    fastcdc::v2020::MINIMUM_MAX,
                    fastcdc::v2020::AVERAGE_MIN,
                    fastcdc::v2020::AVERAGE_MAX,
                    fastcdc::v2020::MAXIMUM_MIN,
                ),
            });
        }
    } else {
        let min = settings.chunking.min_bytes as usize;
        let avg = settings.chunking.avg_bytes as usize;
        let max = settings.chunking.max_bytes as usize;
        let min_ok = (fastcdc::ronomon::MINIMUM_MIN..=fastcdc::ronomon::MINIMUM_MAX).contains(&min);
        let avg_ok = (fastcdc::ronomon::AVERAGE_MIN..=fastcdc::ronomon::AVERAGE_MAX).contains(&avg);
        let max_ok = (fastcdc::ronomon::MAXIMUM_MIN..=fastcdc::ronomon::MAXIMUM_MAX).contains(&max);
        if !(min_ok && avg_ok && max_ok) {
            return Err(Error::InvalidConfig {
                message: format!(
                    "chunk sizes out of bounds for fastcdc::ronomon (min={}..={}, avg={}..={}, max={}..={})",
                    fastcdc::ronomon::MINIMUM_MIN,
                    fastcdc::ronomon::MINIMUM_MAX,
                    fastcdc::ronomon::AVERAGE_MIN,
                    fastcdc::ronomon::AVERAGE_MAX,
                    fastcdc::ronomon::MAXIMUM_MIN,
                    fastcdc::ronomon::MAXIMUM_MAX,
                ),
            });
        }
    }

    // MTProto-only: cap chunking.max_bytes to keep upload_document bytes <= engineered max.
    // upload_bytes = chunk_plain_bytes + framing_overhead_bytes
    let mtproto_max_plain_bytes =
        MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES.saturating_sub(FRAMING_OVERHEAD_BYTES);
    if settings.chunking.max_bytes as usize > mtproto_max_plain_bytes {
        return Err(Error::InvalidConfig {
            message: format!(
                "chunking.max_bytes too large for telegram.mode=\"mtproto\": max_bytes={} must be <= {} (= MTProtoEngineeredUploadMaxBytes {} - framing_overhead {} bytes)",
                settings.chunking.max_bytes,
                mtproto_max_plain_bytes,
                MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES,
                FRAMING_OVERHEAD_BYTES,
            ),
        });
    }

    // Schedule: validate kind + time formats to avoid silently skipping runs.
    validate_schedule_fields(
        "schedule",
        Some(&settings.schedule.kind),
        Some(settings.schedule.hourly_minute),
        Some(&settings.schedule.daily_at),
    )?;

    // Endpoints: unique ids + minimal invariants.
    let mut endpoint_ids = std::collections::HashSet::<String>::new();
    for ep in &settings.telegram_endpoints {
        if ep.id.trim().is_empty() {
            return Err(Error::InvalidConfig {
                message: "telegram_endpoints[].id must not be empty".to_string(),
            });
        }
        if !endpoint_ids.insert(ep.id.clone()) {
            return Err(Error::InvalidConfig {
                message: format!("duplicate telegram_endpoints id: {}", ep.id),
            });
        }
        if ep.mode.trim() != "mtproto" {
            return Err(Error::InvalidConfig {
                message: format!(
                    "telegram_endpoints[].mode must be \"mtproto\" (endpoint_id={})",
                    ep.id
                ),
            });
        }
        if ep.bot_token_key.trim().is_empty() {
            return Err(Error::InvalidConfig {
                message: format!(
                    "telegram_endpoints[].bot_token_key must not be empty (endpoint_id={})",
                    ep.id
                ),
            });
        }
        if ep.mtproto.session_key.trim().is_empty() {
            return Err(Error::InvalidConfig {
                message: format!(
                    "telegram_endpoints[].mtproto.session_key must not be empty (endpoint_id={})",
                    ep.id
                ),
            });
        }
    }

    // Targets: unique ids + endpoint references.
    let mut target_ids = std::collections::HashSet::<String>::new();
    for t in &settings.targets {
        if t.id.trim().is_empty() {
            return Err(Error::InvalidConfig {
                message: "targets[].id must not be empty".to_string(),
            });
        }
        if !target_ids.insert(t.id.clone()) {
            return Err(Error::InvalidConfig {
                message: format!("duplicate target id: {}", t.id),
            });
        }
        if t.source_path.trim().is_empty() {
            return Err(Error::InvalidConfig {
                message: format!(
                    "targets[].source_path must not be empty (target_id={})",
                    t.id
                ),
            });
        }
        if t.endpoint_id.trim().is_empty() {
            return Err(Error::InvalidConfig {
                message: format!(
                    "targets[].endpoint_id must not be empty (target_id={})",
                    t.id
                ),
            });
        }
        if !endpoint_ids.contains(&t.endpoint_id) {
            return Err(Error::InvalidConfig {
                message: format!(
                    "targets[].endpoint_id references unknown endpoint_id={} (target_id={})",
                    t.endpoint_id, t.id
                ),
            });
        }

        if let Some(o) = &t.schedule {
            validate_schedule_fields(
                &format!("targets[].schedule (target_id={})", t.id),
                o.kind.as_ref(),
                o.hourly_minute,
                o.daily_at.as_ref(),
            )?;
        }
    }

    Ok(())
}

fn validate_schedule_fields(
    ctx: &str,
    kind: Option<&String>,
    hourly_minute: Option<u8>,
    daily_at: Option<&String>,
) -> Result<()> {
    if let Some(kind) = kind {
        let k = kind.trim();
        if k != "hourly" && k != "daily" {
            return Err(Error::InvalidConfig {
                message: format!("{ctx}.kind must be \"hourly\" or \"daily\" (got {k:?})"),
            });
        }
    }

    if let Some(minute) = hourly_minute
        && minute >= 60
    {
        return Err(Error::InvalidConfig {
            message: format!("{ctx}.hourly_minute must be 0..=59 (got {minute})"),
        });
    }

    if let Some(daily_at) = daily_at {
        let s = daily_at.trim();
        let (hh, mm) = s.split_once(':').ok_or_else(|| Error::InvalidConfig {
            message: format!("{ctx}.daily_at must be HH:MM (got {s:?})"),
        })?;
        let hh: u8 = hh.parse().map_err(|_| Error::InvalidConfig {
            message: format!("{ctx}.daily_at must be HH:MM (got {s:?})"),
        })?;
        let mm: u8 = mm.parse().map_err(|_| Error::InvalidConfig {
            message: format!("{ctx}.daily_at must be HH:MM (got {s:?})"),
        })?;
        if hh >= 24 || mm >= 60 {
            return Err(Error::InvalidConfig {
                message: format!("{ctx}.daily_at must be HH:MM (got {s:?})"),
            });
        }
    }

    Ok(())
}

pub fn effective_schedule(
    global: &Schedule,
    override_: Option<&TargetScheduleOverride>,
) -> Schedule {
    let mut out = global.clone();
    let Some(o) = override_ else {
        return out;
    };

    if let Some(v) = o.enabled {
        out.enabled = v;
    }
    if let Some(v) = &o.kind {
        out.kind = v.clone();
    }
    if let Some(v) = o.hourly_minute {
        out.hourly_minute = v;
    }
    if let Some(v) = &o.daily_at {
        out.daily_at = v.clone();
    }
    out
}

pub fn endpoint_provider(endpoint_id: &str) -> String {
    format!("telegram.mtproto/{endpoint_id}")
}

pub fn endpoint_bot_token_key_default(endpoint_id: &str) -> String {
    format!("telegram.bot_token.{endpoint_id}")
}

pub fn endpoint_session_key_default(endpoint_id: &str) -> String {
    format!("telegram.mtproto.session.{endpoint_id}")
}

pub fn target_id_from_source_path(source_path: &str) -> String {
    let hash = blake3::hash(source_path.as_bytes()).to_hex();
    format!("src_{}", &hash[..8])
}

fn migrate_v1_to_v2(v1: SettingsV1) -> SettingsV2 {
    let endpoint_id = "default".to_string();

    let endpoints = vec![TelegramEndpoint {
        id: endpoint_id.clone(),
        mode: "mtproto".to_string(),
        chat_id: v1.telegram.chat_id,
        bot_token_key: v1.telegram.bot_token_key,
        mtproto: TelegramEndpointMtproto {
            session_key: v1.telegram.mtproto.session_key,
        },
        rate_limit: v1.telegram.rate_limit,
    }];

    let targets = v1
        .sources
        .into_iter()
        .map(|source_path| Target {
            id: target_id_from_source_path(&source_path),
            source_path,
            label: "manual".to_string(),
            endpoint_id: endpoint_id.clone(),
            enabled: true,
            schedule: None,
        })
        .collect::<Vec<_>>();

    SettingsV2 {
        version: SETTINGS_SCHEMA_VERSION,
        schedule: v1.schedule,
        retention: v1.retention,
        chunking: v1.chunking,
        telegram: TelegramGlobal {
            mode: v1.telegram.mode,
            mtproto: TelegramMtprotoGlobal {
                api_id: v1.telegram.mtproto.api_id,
                api_hash_key: v1.telegram.mtproto.api_hash_key,
            },
        },
        telegram_endpoints: endpoints,
        targets,
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::FRAMING_OVERHEAD_BYTES;
    use crate::storage::MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES;

    fn base_settings_v2() -> SettingsV2 {
        let input = r#"
version = 2

[[telegram_endpoints]]
id = "e1"
mode = "mtproto"
chat_id = "-100123"
bot_token_key = "telegram.bot_token.e1"

[telegram_endpoints.mtproto]
session_key = "telegram.mtproto.session.e1"

[[targets]]
id = "t1"
source_path = "/tmp"
endpoint_id = "e1"
"#;
        parse_settings_v2(input).unwrap()
    }

    #[test]
    fn v1_is_migrated_to_v2() {
        let input = r#"
sources = ["/A", "/B"]

[schedule]
enabled = true
kind = "hourly"
hourly_minute = 0
daily_at = "02:00"
timezone = "local"

[retention]
keep_last_snapshots = 7

[chunking]
min_bytes = 1048576
avg_bytes = 4194304
max_bytes = 10485760

[telegram]
mode = "mtproto"
chat_id = "-100123"
bot_token_key = "telegram.bot_token"

[telegram.mtproto]
api_id = 123
api_hash_key = "telegram.mtproto.api_hash"
session_key = "telegram.mtproto.session"

[telegram.rate_limit]
max_concurrent_uploads = 2
min_delay_ms = 250
"#;
        let s = parse_settings_v2(input).unwrap();
        assert_eq!(s.version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(s.telegram_endpoints.len(), 1);
        assert_eq!(s.telegram_endpoints[0].id, "default");
        assert_eq!(s.targets.len(), 2);
        assert_eq!(s.targets[0].endpoint_id, "default");
    }

    #[test]
    fn v2_schedule_kind_is_validated() {
        let mut s = base_settings_v2();
        s.schedule.enabled = true;
        s.schedule.kind = "weird".to_string();
        let err = validate_settings_schema_v2(&s).unwrap_err();
        assert!(err.to_string().contains("schedule.kind"));
    }

    #[test]
    fn v2_hourly_minute_is_validated() {
        let mut s = base_settings_v2();
        s.schedule.enabled = true;
        s.schedule.kind = "hourly".to_string();
        s.schedule.hourly_minute = 60;
        let err = validate_settings_schema_v2(&s).unwrap_err();
        assert!(err.to_string().contains("hourly_minute"));
    }

    #[test]
    fn v2_daily_at_is_validated() {
        let mut s = base_settings_v2();
        s.schedule.enabled = true;
        s.schedule.kind = "daily".to_string();
        s.schedule.daily_at = "99:99".to_string();
        let err = validate_settings_schema_v2(&s).unwrap_err();
        assert!(err.to_string().contains("daily_at"));
    }

    #[test]
    fn v2_target_schedule_override_is_validated() {
        let mut s = base_settings_v2();
        s.targets[0].schedule = Some(TargetScheduleOverride {
            enabled: Some(true),
            kind: Some("hourly".to_string()),
            hourly_minute: Some(60),
            daily_at: None,
        });
        let err = validate_settings_schema_v2(&s).unwrap_err();
        assert!(err.to_string().contains("targets[].schedule"));
    }

    #[test]
    fn v2_endpoint_mtproto_session_key_defaults_per_endpoint() {
        let input = r#"
version = 2

[[telegram_endpoints]]
id = "e1"
mode = "mtproto"
chat_id = "-100123"
bot_token_key = "telegram.bot_token.e1"

[[telegram_endpoints]]
id = "e2"
mode = "mtproto"
chat_id = "-100456"
bot_token_key = "telegram.bot_token.e2"

[[targets]]
id = "t1"
source_path = "/tmp"
endpoint_id = "e1"
"#;
        let s = parse_settings_v2(input).unwrap();
        assert_eq!(s.telegram_endpoints.len(), 2);
        assert_eq!(
            s.telegram_endpoints[0].mtproto.session_key,
            "telegram.mtproto.session.e1"
        );
        assert_eq!(
            s.telegram_endpoints[1].mtproto.session_key,
            "telegram.mtproto.session.e2"
        );
        validate_settings_schema_v2(&s).unwrap();
    }

    #[test]
    fn v2_chunking_max_bytes_cap_is_validated() {
        let mut s = base_settings_v2();
        s.chunking.min_bytes = 64;
        s.chunking.avg_bytes = 256;

        let max_plain =
            MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES.saturating_sub(FRAMING_OVERHEAD_BYTES) as u32;
        s.chunking.max_bytes = max_plain + 1;

        let err = validate_settings_schema_v2(&s).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("MTProtoEngineeredUploadMaxBytes"));
        assert!(msg.contains("framing_overhead"));
        assert!(msg.contains("41"));
    }

    #[test]
    fn v2_chunking_max_bytes_cap_allows_exact_limit() {
        let mut s = base_settings_v2();
        s.chunking.min_bytes = 64;
        s.chunking.avg_bytes = 256;

        let max_plain =
            MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES.saturating_sub(FRAMING_OVERHEAD_BYTES) as u32;
        s.chunking.max_bytes = max_plain;

        validate_settings_schema_v2(&s).unwrap();
    }

    #[test]
    fn v2_chunking_min_avg_max_relation_is_validated() {
        let mut s = base_settings_v2();
        s.chunking.min_bytes = 1024 * 1024;
        s.chunking.avg_bytes = 512 * 1024;
        s.chunking.max_bytes = 2 * 1024 * 1024;
        let err = validate_settings_schema_v2(&s).unwrap_err();
        assert!(err.to_string().contains("min <= avg <= max"));
    }
}
