use std::collections::{BTreeMap, BTreeSet};

use base64::Engine;
use pbkdf2::pbkdf2_hmac;
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use crate::config::{SETTINGS_SCHEMA_VERSION, SettingsV2};
use crate::crypto::{decrypt_framed, encrypt_framed};
use crate::{Error, Result, gold_key};

pub const CONFIG_BUNDLE_PREFIX_V2: &str = "TBC2:";
pub const CONFIG_BUNDLE_FORMAT_V2: &str = "tbc2";
pub const CONFIG_BUNDLE_VERSION_V2: u32 = 2;
pub const CONFIG_BUNDLE_KDF_V2: &str = "pbkdf2_hmac_sha256";
pub const CONFIG_BUNDLE_KDF_SALT_LEN_V2: usize = 16;
pub const CONFIG_BUNDLE_KDF_ITERS_V2: u32 = 200_000;
pub const CONFIG_BUNDLE_KDF_MAX_ITERS_V2: u32 = 1_000_000;
pub const CONFIG_BUNDLE_AAD_GOLD_KEY_V2: &[u8] = b"televy.config.bundle.v2.gold_key";
pub const CONFIG_BUNDLE_AAD_PAYLOAD_V2: &[u8] = b"televy.config.bundle.v2.payload";

const CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY: &str = "televybackup.master_key";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigBundleOuterV2 {
    pub version: u32,
    pub format: String,
    pub hint: String,
    pub kdf: ConfigBundleKdfV2,
    pub gold_key_enc: String,
    pub payload_enc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ConfigBundleSecretsV2 {
    pub entries: BTreeMap<String, String>,
    #[serde(default)]
    pub excluded: Vec<String>,
    #[serde(default)]
    pub missing: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigBundlePayloadV2 {
    pub version: u32,
    pub exported_at: String,
    pub settings: SettingsV2,
    pub secrets: ConfigBundleSecretsV2,
}

#[derive(Debug, Clone)]
pub struct DecodedConfigBundleV2 {
    pub master_key: [u8; 32],
    pub outer: ConfigBundleOuterV2,
    pub payload: ConfigBundlePayloadV2,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigBundleKdfV2 {
    pub name: String,
    pub iterations: u32,
    pub salt: String,
}

fn derive_bundle_passphrase_key_v2(
    passphrase: &str,
    salt: &[u8],
    iterations: u32,
) -> Result<[u8; 32]> {
    if passphrase.trim().is_empty() {
        return Err(Error::InvalidConfig {
            message: "config bundle passphrase must not be empty".to_string(),
        });
    }
    if salt.is_empty() {
        return Err(Error::InvalidConfig {
            message: "config bundle salt must not be empty".to_string(),
        });
    }
    if iterations < 10_000 {
        return Err(Error::InvalidConfig {
            message: "config bundle KDF iterations too small".to_string(),
        });
    }
    if iterations > CONFIG_BUNDLE_KDF_MAX_ITERS_V2 {
        return Err(Error::InvalidConfig {
            message: format!("config bundle KDF iterations too large: {iterations}"),
        });
    }

    let mut out = [0u8; 32];
    pbkdf2_hmac::<Sha256>(passphrase.as_bytes(), salt, iterations, &mut out);
    Ok(out)
}

fn validate_bundle_secrets_v2(
    settings: &SettingsV2,
    secrets: &ConfigBundleSecretsV2,
) -> Result<()> {
    let mut required_keys = BTreeSet::<String>::new();
    required_keys.insert(settings.telegram.mtproto.api_hash_key.clone());
    for ep in &settings.telegram_endpoints {
        required_keys.insert(ep.bot_token_key.clone());
    }

    let mut excluded_keys = BTreeSet::<String>::new();
    for ep in &settings.telegram_endpoints {
        excluded_keys.insert(ep.mtproto.session_key.clone());
    }

    // Reject bundles that try to smuggle reserved keys into the secrets store.
    if required_keys.contains(CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY)
        || excluded_keys.contains(CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY)
    {
        return Err(Error::InvalidConfig {
            message: format!(
                "config bundle settings may not reference reserved secret key: {CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY}"
            ),
        });
    }

    for key in secrets.entries.keys() {
        let trimmed = key.trim();
        if trimmed.is_empty() || trimmed != key {
            return Err(Error::InvalidConfig {
                message: format!("config bundle secrets.entries key is invalid: {key:?}"),
            });
        }
        if key == CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY {
            return Err(Error::InvalidConfig {
                message: format!(
                    "config bundle secrets.entries must not contain reserved key: {key}"
                ),
            });
        }
        if !required_keys.contains(key) {
            return Err(Error::InvalidConfig {
                message: format!("config bundle secrets.entries contains unknown key: {key}"),
            });
        }
    }

    for key in &secrets.missing {
        let trimmed = key.trim();
        if trimmed.is_empty() || trimmed != key {
            return Err(Error::InvalidConfig {
                message: format!("config bundle secrets.missing key is invalid: {key:?}"),
            });
        }
        if key == CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY {
            return Err(Error::InvalidConfig {
                message: format!(
                    "config bundle secrets.missing must not contain reserved key: {key}"
                ),
            });
        }
        if !required_keys.contains(key) {
            return Err(Error::InvalidConfig {
                message: format!("config bundle secrets.missing contains unknown key: {key}"),
            });
        }
    }

    for key in &secrets.excluded {
        let trimmed = key.trim();
        if trimmed.is_empty() || trimmed != key {
            return Err(Error::InvalidConfig {
                message: format!("config bundle secrets.excluded key is invalid: {key:?}"),
            });
        }
        if key == CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY {
            return Err(Error::InvalidConfig {
                message: format!(
                    "config bundle secrets.excluded must not contain reserved key: {key}"
                ),
            });
        }
        if !excluded_keys.contains(key) {
            return Err(Error::InvalidConfig {
                message: format!("config bundle secrets.excluded contains unknown key: {key}"),
            });
        }
    }

    Ok(())
}

pub fn encode_config_bundle_key_v2(
    master_key: &[u8; 32],
    settings: &SettingsV2,
    secrets: ConfigBundleSecretsV2,
    passphrase: &str,
    hint: &str,
) -> Result<String> {
    let hint = hint.trim();
    if settings.version != SETTINGS_SCHEMA_VERSION {
        return Err(Error::InvalidConfig {
            message: format!(
                "settings schema version mismatch: expected={} got={}",
                SETTINGS_SCHEMA_VERSION, settings.version
            ),
        });
    }
    crate::config::validate_settings_schema_v2(settings)?;
    validate_bundle_secrets_v2(settings, &secrets)?;

    let payload = ConfigBundlePayloadV2 {
        version: CONFIG_BUNDLE_VERSION_V2,
        exported_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        settings: settings.clone(),
        secrets,
    };
    let payload_json = serde_json::to_vec(&payload).map_err(|e| Error::InvalidConfig {
        message: format!("config bundle payload json encode failed: {e}"),
    })?;

    let payload_framed = encrypt_framed(master_key, CONFIG_BUNDLE_AAD_PAYLOAD_V2, &payload_json)?;

    let mut salt = [0u8; CONFIG_BUNDLE_KDF_SALT_LEN_V2];
    getrandom::getrandom(&mut salt).map_err(|e| Error::InvalidConfig {
        message: format!("getrandom failed: {e}"),
    })?;

    let key = derive_bundle_passphrase_key_v2(passphrase, &salt, CONFIG_BUNDLE_KDF_ITERS_V2)?;
    let gold_key = gold_key::encode_gold_key(master_key);
    let gold_key_framed = encrypt_framed(&key, CONFIG_BUNDLE_AAD_GOLD_KEY_V2, gold_key.as_bytes())?;

    let outer = ConfigBundleOuterV2 {
        version: CONFIG_BUNDLE_VERSION_V2,
        format: CONFIG_BUNDLE_FORMAT_V2.to_string(),
        hint: hint.to_string(),
        kdf: ConfigBundleKdfV2 {
            name: CONFIG_BUNDLE_KDF_V2.to_string(),
            iterations: CONFIG_BUNDLE_KDF_ITERS_V2,
            salt: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(salt),
        },
        gold_key_enc: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(gold_key_framed),
        payload_enc: base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload_framed),
    };
    let outer_json = serde_json::to_vec(&outer).map_err(|e| Error::InvalidConfig {
        message: format!("config bundle outer json encode failed: {e}"),
    })?;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(outer_json);
    Ok(format!("{CONFIG_BUNDLE_PREFIX_V2}{b64}"))
}

pub fn decode_config_bundle_key_v2(s: &str, passphrase: &str) -> Result<DecodedConfigBundleV2> {
    let rest = s
        .trim()
        .strip_prefix(CONFIG_BUNDLE_PREFIX_V2)
        .ok_or_else(|| Error::InvalidConfig {
            message: format!("invalid config bundle (missing {CONFIG_BUNDLE_PREFIX_V2} prefix)"),
        })?;

    if rest.contains('+') || rest.contains('@') {
        return Err(Error::InvalidConfig {
            message: "invalid config bundle (contains '+' or '@')".to_string(),
        });
    }

    let outer_json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(rest.as_bytes())
        .map_err(|e| Error::InvalidConfig {
            message: format!("invalid config bundle (bad base64url): {e}"),
        })?;

    let outer: ConfigBundleOuterV2 =
        serde_json::from_slice(&outer_json).map_err(|e| Error::InvalidConfig {
            message: format!("invalid config bundle outer json: {e}"),
        })?;

    if outer.version != CONFIG_BUNDLE_VERSION_V2 {
        return Err(Error::InvalidConfig {
            message: format!("unsupported config bundle version: {}", outer.version),
        });
    }
    if outer.format != CONFIG_BUNDLE_FORMAT_V2 {
        return Err(Error::InvalidConfig {
            message: format!("invalid config bundle format: {}", outer.format),
        });
    }
    if outer.kdf.name != CONFIG_BUNDLE_KDF_V2 {
        return Err(Error::InvalidConfig {
            message: format!("unsupported config bundle kdf: {}", outer.kdf.name),
        });
    }

    let salt = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(outer.kdf.salt.as_bytes())
        .map_err(|e| Error::InvalidConfig {
            message: format!("invalid config bundle kdf.salt (bad base64url): {e}"),
        })?;
    if salt.len() < CONFIG_BUNDLE_KDF_SALT_LEN_V2 {
        return Err(Error::InvalidConfig {
            message: "invalid config bundle kdf.salt (too small)".to_string(),
        });
    }
    let key = derive_bundle_passphrase_key_v2(passphrase, &salt, outer.kdf.iterations)?;

    let gold_key_framed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(outer.gold_key_enc.as_bytes())
        .map_err(|e| Error::InvalidConfig {
            message: format!("invalid config bundle goldKeyEnc (bad base64url): {e}"),
        })?;
    let gold_key_bytes = decrypt_framed(&key, CONFIG_BUNDLE_AAD_GOLD_KEY_V2, &gold_key_framed)?;
    let gold_key_str = String::from_utf8(gold_key_bytes).map_err(|e| Error::Crypto {
        message: format!("config bundle gold key decode failed: {e}"),
    })?;
    let master_key = gold_key::decode_gold_key(&gold_key_str)?;

    let payload_framed = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(outer.payload_enc.as_bytes())
        .map_err(|e| Error::InvalidConfig {
            message: format!("invalid config bundle payloadEnc (bad base64url): {e}"),
        })?;

    let payload_json = decrypt_framed(&master_key, CONFIG_BUNDLE_AAD_PAYLOAD_V2, &payload_framed)?;
    let payload: ConfigBundlePayloadV2 =
        serde_json::from_slice(&payload_json).map_err(|e| Error::Crypto {
            message: format!("config bundle payload json decode failed: {e}"),
        })?;

    if payload.version != CONFIG_BUNDLE_VERSION_V2 {
        return Err(Error::InvalidConfig {
            message: format!(
                "unsupported config bundle payload version: {}",
                payload.version
            ),
        });
    }
    if payload.settings.version != SETTINGS_SCHEMA_VERSION {
        return Err(Error::InvalidConfig {
            message: format!(
                "config bundle settings schema mismatch: expected={} got={}",
                SETTINGS_SCHEMA_VERSION, payload.settings.version
            ),
        });
    }
    crate::config::validate_settings_schema_v2(&payload.settings)?;
    validate_bundle_secrets_v2(&payload.settings, &payload.secrets)?;

    Ok(DecodedConfigBundleV2 {
        master_key,
        outer,
        payload,
    })
}

pub fn utc_now_compact_timestamp() -> String {
    chrono::Utc::now().format("%Y%m%d-%H%M%S").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{TelegramEndpoint, TelegramEndpointMtproto, TelegramRateLimit};

    fn outer_from_bundle_key(key: &str) -> ConfigBundleOuterV2 {
        let rest = key
            .strip_prefix(CONFIG_BUNDLE_PREFIX_V2)
            .expect("missing TBC2 prefix");
        let outer_json = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(rest.as_bytes())
            .expect("outer base64 decode failed");
        serde_json::from_slice(&outer_json).expect("outer json decode failed")
    }

    fn bundle_key_from_outer(outer: &ConfigBundleOuterV2) -> String {
        let outer_json = serde_json::to_vec(outer).expect("outer json encode failed");
        let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(outer_json);
        format!("{CONFIG_BUNDLE_PREFIX_V2}{b64}")
    }

    fn tamper_payload(
        bundle_key: &str,
        master_key: &[u8; 32],
        f: impl FnOnce(&mut ConfigBundlePayloadV2),
    ) -> String {
        let mut outer = outer_from_bundle_key(bundle_key);
        let payload_framed = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(outer.payload_enc.as_bytes())
            .expect("payloadEnc base64 decode failed");
        let payload_json =
            decrypt_framed(master_key, CONFIG_BUNDLE_AAD_PAYLOAD_V2, &payload_framed).unwrap();
        let mut payload: ConfigBundlePayloadV2 =
            serde_json::from_slice(&payload_json).expect("payload json decode failed");
        f(&mut payload);
        let new_payload_json = serde_json::to_vec(&payload).expect("payload json encode failed");
        let new_payload_framed =
            encrypt_framed(master_key, CONFIG_BUNDLE_AAD_PAYLOAD_V2, &new_payload_json).unwrap();
        outer.payload_enc =
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(new_payload_framed);
        bundle_key_from_outer(&outer)
    }

    fn sample_settings() -> SettingsV2 {
        SettingsV2 {
            version: SETTINGS_SCHEMA_VERSION,
            schedule: crate::config::Schedule::default(),
            retention: crate::config::Retention::default(),
            chunking: crate::config::Chunking::default(),
            telegram: crate::config::TelegramGlobal::default(),
            telegram_endpoints: vec![TelegramEndpoint {
                id: "ep1".to_string(),
                mode: "mtproto".to_string(),
                chat_id: "-1001".to_string(),
                bot_token_key: "telegram.bot_token.ep1".to_string(),
                mtproto: TelegramEndpointMtproto {
                    session_key: "telegram.mtproto.session.ep1".to_string(),
                },
                rate_limit: TelegramRateLimit::default(),
            }],
            targets: vec![crate::config::Target {
                id: "t1".to_string(),
                source_path: "/tmp".to_string(),
                label: "manual".to_string(),
                endpoint_id: "ep1".to_string(),
                enabled: true,
                schedule: None,
            }],
        }
    }

    #[test]
    fn tbc2_round_trip() {
        let master_key = [9u8; 32];
        let settings = sample_settings();

        let mut secrets = ConfigBundleSecretsV2::default();
        secrets
            .entries
            .insert("telegram.mtproto.api_hash".to_string(), "hash".to_string());
        secrets
            .excluded
            .push("telegram.mtproto.session.ep1".to_string());
        secrets.missing.push("telegram.bot_token.ep1".to_string());

        let key = encode_config_bundle_key_v2(
            &master_key,
            &settings,
            secrets.clone(),
            "hunter2",
            "my hint",
        )
        .unwrap();
        assert!(key.starts_with(CONFIG_BUNDLE_PREFIX_V2));

        let dec = decode_config_bundle_key_v2(&key, "hunter2").unwrap();
        assert_eq!(dec.master_key, master_key);
        assert_eq!(dec.payload.version, CONFIG_BUNDLE_VERSION_V2);
        assert_eq!(dec.payload.settings.version, SETTINGS_SCHEMA_VERSION);
        assert_eq!(dec.payload.secrets.entries, secrets.entries);
        assert_eq!(dec.payload.secrets.excluded, secrets.excluded);
        assert_eq!(dec.payload.secrets.missing, secrets.missing);
        assert_eq!(dec.outer.hint, "my hint");
    }

    #[test]
    fn tbc2_decrypt_fails_with_wrong_passphrase() {
        let master_key = [1u8; 32];
        let settings = sample_settings();
        let key = encode_config_bundle_key_v2(
            &master_key,
            &settings,
            ConfigBundleSecretsV2::default(),
            "correct horse",
            "my hint",
        )
        .unwrap();

        let err = decode_config_bundle_key_v2(&key, "wrong pass").unwrap_err();
        assert!(matches!(err, Error::Crypto { .. }));
    }

    #[test]
    fn tbc2_round_trip_allows_empty_hint() {
        let master_key = [9u8; 32];
        let settings = sample_settings();

        let key = encode_config_bundle_key_v2(
            &master_key,
            &settings,
            ConfigBundleSecretsV2::default(),
            "hunter2",
            "",
        )
        .unwrap();
        assert!(key.starts_with(CONFIG_BUNDLE_PREFIX_V2));

        let dec = decode_config_bundle_key_v2(&key, "hunter2").unwrap();
        assert_eq!(dec.outer.hint, "");
    }

    #[test]
    fn tbc2_kdf_iterations_too_large_is_rejected() {
        let err = derive_bundle_passphrase_key_v2(
            "hunter2",
            &[0u8; CONFIG_BUNDLE_KDF_SALT_LEN_V2],
            CONFIG_BUNDLE_KDF_MAX_ITERS_V2 + 1,
        )
        .unwrap_err();
        assert!(matches!(err, Error::InvalidConfig { .. }));
        assert!(err.to_string().contains("iterations too large"));
    }

    #[test]
    fn tbc2_decode_rejects_unknown_secret_key() {
        let master_key = [9u8; 32];
        let settings = sample_settings();
        let key = encode_config_bundle_key_v2(
            &master_key,
            &settings,
            ConfigBundleSecretsV2::default(),
            "hunter2",
            "hint",
        )
        .unwrap();

        let tampered = tamper_payload(&key, &master_key, |payload| {
            payload
                .secrets
                .entries
                .insert("unknown.secret.key".to_string(), "x".to_string());
        });

        let err = decode_config_bundle_key_v2(&tampered, "hunter2").unwrap_err();
        assert!(matches!(err, Error::InvalidConfig { .. }));
        assert!(err.to_string().contains("unknown key"));
    }

    #[test]
    fn tbc2_decode_rejects_master_key_in_secrets_entries() {
        let master_key = [9u8; 32];
        let settings = sample_settings();
        let key = encode_config_bundle_key_v2(
            &master_key,
            &settings,
            ConfigBundleSecretsV2::default(),
            "hunter2",
            "hint",
        )
        .unwrap();

        let tampered = tamper_payload(&key, &master_key, |payload| {
            payload.secrets.entries.insert(
                CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY.to_string(),
                "evil".to_string(),
            );
        });

        let err = decode_config_bundle_key_v2(&tampered, "hunter2").unwrap_err();
        assert!(matches!(err, Error::InvalidConfig { .. }));
        assert!(err.to_string().contains("reserved"));
    }

    #[test]
    fn tbc2_decode_rejects_settings_reference_to_reserved_secret_key() {
        let master_key = [9u8; 32];
        let settings = sample_settings();
        let key = encode_config_bundle_key_v2(
            &master_key,
            &settings,
            ConfigBundleSecretsV2::default(),
            "hunter2",
            "hint",
        )
        .unwrap();

        let tampered = tamper_payload(&key, &master_key, |payload| {
            payload.settings.telegram.mtproto.api_hash_key =
                CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY.to_string();
            payload.secrets.entries.insert(
                CONFIG_BUNDLE_RESERVED_MASTER_KEY_KEY.to_string(),
                "evil".to_string(),
            );
        });

        let err = decode_config_bundle_key_v2(&tampered, "hunter2").unwrap_err();
        assert!(matches!(err, Error::InvalidConfig { .. }));
        assert!(
            err.to_string()
                .contains("may not reference reserved secret key")
        );
    }
}
