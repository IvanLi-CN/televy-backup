use serde::{Deserialize, Serialize};

use crate::crypto::{decrypt_framed, encrypt_framed};
use crate::storage::Storage;
use crate::{Error, Result};

pub const BOOTSTRAP_CATALOG_VERSION: u32 = 1;
pub const BOOTSTRAP_CATALOG_AAD: &[u8] = b"televy.bootstrap.catalog.v1";

pub trait PinnedStorage: Storage {
    fn get_pinned_object_id(&self) -> Result<Option<String>>;
    fn set_pinned_object_id(&self, object_id: &str) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapCatalogV1 {
    pub version: u32,
    pub updated_at: String,
    pub targets: Vec<BootstrapTarget>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapTarget {
    pub target_id: String,
    pub source_path: String,
    pub label: String,
    #[serde(default)]
    pub latest: Option<BootstrapLatest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BootstrapLatest {
    pub snapshot_id: String,
    pub manifest_object_id: String,
}

impl Default for BootstrapCatalogV1 {
    fn default() -> Self {
        Self {
            version: BOOTSTRAP_CATALOG_VERSION,
            updated_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            targets: Vec::new(),
        }
    }
}

pub fn encrypt_catalog(master_key: &[u8; 32], catalog: &BootstrapCatalogV1) -> Result<Vec<u8>> {
    if catalog.version != BOOTSTRAP_CATALOG_VERSION {
        return Err(Error::InvalidConfig {
            message: "invalid bootstrap catalog version".to_string(),
        });
    }
    let json = serde_json::to_vec(catalog).map_err(|_| Error::InvalidConfig {
        message: "bootstrap catalog json encode failed".to_string(),
    })?;
    encrypt_framed(master_key, BOOTSTRAP_CATALOG_AAD, &json)
}

pub fn decrypt_catalog(master_key: &[u8; 32], framed: &[u8]) -> Result<BootstrapCatalogV1> {
    let json = decrypt_framed(master_key, BOOTSTRAP_CATALOG_AAD, framed)?;
    let cat: BootstrapCatalogV1 = serde_json::from_slice(&json).map_err(|_| Error::Crypto {
        message: "bootstrap catalog json decode failed".to_string(),
    })?;
    if cat.version != BOOTSTRAP_CATALOG_VERSION {
        return Err(Error::InvalidConfig {
            message: "bootstrap catalog version mismatch".to_string(),
        });
    }
    Ok(cat)
}

pub async fn load_remote_catalog<S: PinnedStorage>(
    storage: &S,
    master_key: &[u8; 32],
) -> Result<Option<BootstrapCatalogV1>> {
    let Some(object_id) = storage.get_pinned_object_id()? else {
        return Ok(None);
    };
    let bytes = storage.download_document(&object_id).await?;
    match decrypt_catalog(master_key, &bytes) {
        Ok(cat) => Ok(Some(cat)),
        Err(e) => {
            // A pinned message may exist but not belong to TelevyBackup (or may be encrypted with a
            // different key). Don't block normal setup: treat it as "no catalog" and let the next
            // update overwrite the pinned item.
            tracing::warn!(
                event = "bootstrap.catalog.decrypt_failed",
                object_id = %object_id,
                error = %e,
                "ignoring pinned document: not a decryptable TelevyBackup bootstrap catalog"
            );
            Ok(None)
        }
    }
}

pub async fn save_remote_catalog<S: PinnedStorage>(
    storage: &S,
    master_key: &[u8; 32],
    catalog: &BootstrapCatalogV1,
) -> Result<String> {
    let bytes = encrypt_catalog(master_key, catalog)?;
    let object_id = storage
        .upload_document("televybackup-bootstrap.catalog", bytes)
        .await?;
    storage.set_pinned_object_id(&object_id)?;
    Ok(object_id)
}

pub async fn update_remote_latest<S: PinnedStorage>(
    storage: &S,
    master_key: &[u8; 32],
    target_id: &str,
    source_path: &str,
    label: &str,
    snapshot_id: &str,
    manifest_object_id: &str,
) -> Result<()> {
    let mut cat = load_remote_catalog(storage, master_key)
        .await?
        .unwrap_or_default();
    cat.updated_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let mut found = false;
    for t in &mut cat.targets {
        if t.target_id == target_id {
            t.source_path = source_path.to_string();
            t.label = label.to_string();
            t.latest = Some(BootstrapLatest {
                snapshot_id: snapshot_id.to_string(),
                manifest_object_id: manifest_object_id.to_string(),
            });
            found = true;
            break;
        }
    }

    if !found {
        cat.targets.push(BootstrapTarget {
            target_id: target_id.to_string(),
            source_path: source_path.to_string(),
            label: label.to_string(),
            latest: Some(BootstrapLatest {
                snapshot_id: snapshot_id.to_string(),
                manifest_object_id: manifest_object_id.to_string(),
            }),
        });
    }

    let _ = save_remote_catalog(storage, master_key, &cat).await?;
    Ok(())
}

pub async fn resolve_remote_latest<S: PinnedStorage>(
    storage: &S,
    master_key: &[u8; 32],
    target_id: Option<&str>,
    source_path: Option<&str>,
) -> Result<BootstrapLatest> {
    let cat = load_remote_catalog(storage, master_key)
        .await?
        .ok_or_else(|| Error::BootstrapMissing {
            message: "no pinned bootstrap catalog".to_string(),
        })?;

    if let Some(id) = target_id {
        let t = cat
            .targets
            .into_iter()
            .find(|t| t.target_id == id)
            .ok_or_else(|| Error::InvalidConfig {
                message: format!("bootstrap missing target_id: {id}"),
            })?;
        return t.latest.ok_or_else(|| Error::InvalidConfig {
            message: format!("bootstrap missing latest for target_id: {id}"),
        });
    }

    let Some(source) = source_path else {
        return Err(Error::InvalidConfig {
            message: "must provide target_id or source_path".to_string(),
        });
    };

    let matches = cat
        .targets
        .into_iter()
        .filter(|t| t.source_path == source)
        .collect::<Vec<_>>();

    if matches.is_empty() {
        return Err(Error::InvalidConfig {
            message: format!("bootstrap missing source_path: {source}"),
        });
    }
    if matches.len() > 1 {
        return Err(Error::InvalidConfig {
            message: format!("bootstrap source_path is ambiguous: {source}"),
        });
    }
    matches[0]
        .latest
        .clone()
        .ok_or_else(|| Error::InvalidConfig {
            message: format!("bootstrap missing latest for source_path: {source}"),
        })
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::storage::InMemoryStorage;

    struct MemPinned {
        inner: InMemoryStorage,
        pinned: Mutex<Option<String>>,
    }

    impl MemPinned {
        fn new() -> Self {
            Self {
                inner: InMemoryStorage::new(),
                pinned: Mutex::new(None),
            }
        }
    }

    impl Storage for MemPinned {
        fn provider(&self) -> &str {
            self.inner.provider()
        }

        fn upload_document<'a>(
            &'a self,
            filename: &'a str,
            bytes: Vec<u8>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>>
        {
            self.inner.upload_document(filename, bytes)
        }

        fn download_document<'a>(
            &'a self,
            object_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>>> + Send + 'a>>
        {
            self.inner.download_document(object_id)
        }
    }

    impl PinnedStorage for MemPinned {
        fn get_pinned_object_id(&self) -> Result<Option<String>> {
            Ok(self.pinned.lock().ok().and_then(|g| g.clone()))
        }

        fn set_pinned_object_id(&self, object_id: &str) -> Result<()> {
            *self.pinned.lock().map_err(|_| Error::InvalidConfig {
                message: "lock poisoned".to_string(),
            })? = Some(object_id.to_string());
            Ok(())
        }
    }

    #[tokio::test]
    async fn catalog_round_trip_via_remote() {
        let store = MemPinned::new();
        let key = [3u8; 32];

        update_remote_latest(&store, &key, "t1", "/A", "manual", "snp_1", "obj_1")
            .await
            .unwrap();

        let latest = resolve_remote_latest(&store, &key, Some("t1"), None)
            .await
            .unwrap();
        assert_eq!(latest.snapshot_id, "snp_1");
        assert_eq!(latest.manifest_object_id, "obj_1");
    }

    #[tokio::test]
    async fn update_overwrites_non_catalog_pinned_doc() {
        let store = MemPinned::new();
        let key = [3u8; 32];

        let pinned_before = store
            .upload_document("unrelated-pinned", b"not a catalog".to_vec())
            .await
            .unwrap();
        store.set_pinned_object_id(&pinned_before).unwrap();

        update_remote_latest(&store, &key, "t1", "/A", "manual", "snp_1", "obj_1")
            .await
            .unwrap();

        let pinned_after = store.get_pinned_object_id().unwrap().unwrap();
        assert_ne!(pinned_after, pinned_before);

        let latest = resolve_remote_latest(&store, &key, Some("t1"), None)
            .await
            .unwrap();
        assert_eq!(latest.snapshot_id, "snp_1");
        assert_eq!(latest.manifest_object_id, "obj_1");
    }
}
