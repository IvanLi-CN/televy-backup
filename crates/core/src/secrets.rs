use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use base64::Engine;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

pub const SECRETS_FILE_NAME: &str = "secrets.enc";
pub const VAULT_KEY_KEY: &str = "televybackup.vault_key";
pub const VAULT_KEY_FILE_NAME: &str = "vault.key";

const SECRETS_FILE_VERSION: u8 = 1;
const SECRETS_PAYLOAD_VERSION: u32 = 1;
const SECRETS_AD: &[u8] = b"televybackup.secrets.v1";

#[derive(Debug, thiserror::Error)]
pub enum SecretsStoreError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("random error: {message}")]
    Random { message: String },

    #[error("crypto error")]
    Crypto,

    #[error("invalid secrets store: {message}")]
    InvalidFormat { message: String },

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("base64 error: {0}")]
    Base64(#[from] base64::DecodeError),
}

impl From<getrandom::Error> for SecretsStoreError {
    fn from(e: getrandom::Error) -> Self {
        Self::Random {
            message: e.to_string(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SecretsStore {
    entries: BTreeMap<String, String>,
}

impl SecretsStore {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries.get(key).map(|s| s.as_str())
    }

    pub fn contains_key(&self, key: &str) -> bool {
        self.entries.contains_key(key)
    }

    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.insert(key.into(), value.into());
    }

    pub fn remove(&mut self, key: &str) -> bool {
        self.entries.remove(key).is_some()
    }

    pub fn keys(&self) -> impl Iterator<Item = &String> {
        self.entries.keys()
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct SecretsPayloadV1 {
    version: u32,
    entries: BTreeMap<String, String>,
}

pub fn vault_key_from_base64(b64: &str) -> Result<[u8; 32], SecretsStoreError> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?;
    bytes
        .try_into()
        .map_err(|_| SecretsStoreError::InvalidFormat {
            message: "vault key must be exactly 32 bytes".to_string(),
        })
}

pub fn vault_key_to_base64(vault_key: &[u8; 32]) -> String {
    base64::engine::general_purpose::STANDARD.encode(vault_key)
}

pub fn secrets_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SECRETS_FILE_NAME)
}

pub fn vault_key_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(VAULT_KEY_FILE_NAME)
}

pub fn read_vault_key_file(path: &Path) -> Result<[u8; 32], SecretsStoreError> {
    let text = std::fs::read_to_string(path)?;
    vault_key_from_base64(text.trim())
}

pub fn write_vault_key_file_private(
    path: &Path,
    vault_key: &[u8; 32],
) -> Result<(), SecretsStoreError> {
    if let Some(parent) = path.parent() {
        ensure_private_dir(parent)?;
    }

    let b64 = vault_key_to_base64(vault_key);
    write_atomic_private_text(path, &(b64 + "\n"))?;
    Ok(())
}

pub fn vault_ipc_socket_path(data_dir: &Path) -> PathBuf {
    data_dir.join("ipc").join("vault.sock")
}

pub fn load_secrets_store(
    path: &Path,
    vault_key: &[u8; 32],
) -> Result<SecretsStore, SecretsStoreError> {
    if !path.exists() {
        return Ok(SecretsStore::default());
    }

    let bytes = std::fs::read(path)?;
    decrypt_secrets_store_bytes(vault_key, &bytes)
}

pub fn save_secrets_store(
    path: &Path,
    vault_key: &[u8; 32],
    store: &SecretsStore,
) -> Result<(), SecretsStoreError> {
    let bytes = encrypt_secrets_store_bytes(vault_key, store)?;
    write_atomic_private(path, &bytes)?;
    Ok(())
}

fn decrypt_secrets_store_bytes(
    vault_key: &[u8; 32],
    bytes: &[u8],
) -> Result<SecretsStore, SecretsStoreError> {
    if bytes.len() < 1 + 24 {
        return Err(SecretsStoreError::InvalidFormat {
            message: "secrets store too small".to_string(),
        });
    }
    let version = bytes[0];
    if version != SECRETS_FILE_VERSION {
        return Err(SecretsStoreError::InvalidFormat {
            message: format!("unsupported secrets store version: {version}"),
        });
    }

    let nonce = XNonce::from_slice(&bytes[1..25]);
    let ciphertext = &bytes[25..];

    let cipher = XChaCha20Poly1305::new(vault_key.into());
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: ciphertext,
                aad: SECRETS_AD,
            },
        )
        .map_err(|_| SecretsStoreError::Crypto)?;

    let payload: SecretsPayloadV1 = serde_json::from_slice(&plaintext)?;
    if payload.version != SECRETS_PAYLOAD_VERSION {
        return Err(SecretsStoreError::InvalidFormat {
            message: format!("unsupported secrets payload version: {}", payload.version),
        });
    }

    Ok(SecretsStore {
        entries: payload.entries,
    })
}

fn encrypt_secrets_store_bytes(
    vault_key: &[u8; 32],
    store: &SecretsStore,
) -> Result<Vec<u8>, SecretsStoreError> {
    let payload = SecretsPayloadV1 {
        version: SECRETS_PAYLOAD_VERSION,
        entries: store.entries.clone(),
    };
    let plaintext = serde_json::to_vec(&payload)?;

    let mut nonce_bytes = [0u8; 24];
    getrandom::getrandom(&mut nonce_bytes)?;
    let nonce = XNonce::from_slice(&nonce_bytes);

    let cipher = XChaCha20Poly1305::new(vault_key.into());
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: &plaintext,
                aad: SECRETS_AD,
            },
        )
        .map_err(|_| SecretsStoreError::Crypto)?;

    let mut out = Vec::with_capacity(1 + nonce_bytes.len() + ciphertext.len());
    out.push(SECRETS_FILE_VERSION);
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ciphertext);
    Ok(out)
}

fn write_atomic_private(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp = path.with_extension("tmp");

    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

fn ensure_private_dir(path: &Path) -> Result<(), std::io::Error> {
    if path.exists() {
        return Ok(());
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new().recursive(true).mode(0o700).create(path)?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(path)?;
        Ok(())
    }
}

fn write_atomic_private_text(path: &Path, text: &str) -> Result<(), std::io::Error> {
    let tmp = path.with_extension("tmp");

    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)?;
        f.write_all(text.as_bytes())?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }

    #[cfg(not(unix))]
    {
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_store_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.enc");
        let vault_key = [7u8; 32];

        let mut store = SecretsStore::default();
        store.set("k", "v");
        save_secrets_store(&path, &vault_key, &store).unwrap();

        let loaded = load_secrets_store(&path, &vault_key).unwrap();
        assert_eq!(loaded.get("k"), Some("v"));
    }

    #[test]
    fn vault_key_file_roundtrip_and_trim() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("nested");
        let path = nested.join("vault.key");
        let key = [7u8; 32];

        write_vault_key_file_private(&path, &key).unwrap();
        let loaded = read_vault_key_file(&path).unwrap();
        assert_eq!(loaded, key);

        let b64 = vault_key_to_base64(&key);
        std::fs::write(&path, format!(" \n{b64}\n \n")).unwrap();
        let loaded = read_vault_key_file(&path).unwrap();
        assert_eq!(loaded, key);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(file_mode, 0o600);

            let dir_mode = std::fs::metadata(&nested).unwrap().permissions().mode() & 0o777;
            assert_eq!(dir_mode, 0o700);
        }
    }
}
