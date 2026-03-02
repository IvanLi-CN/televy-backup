use serde::{Deserialize, Serialize};

use crate::crypto::{decrypt_framed, encrypt_framed};
use crate::storage::Storage;
use crate::{Error, Result};

pub const DEDUPE_CATALOG_VERSION: u32 = 1;

pub const ENDPOINT_DEDUPE_ID_PREFIX_V1: &str = "televy.endpoint_dedupe.v1:";
pub const DEDUPE_BASE_ID_PREFIX_V1: &str = "televy.endpoint_dedupe.base.v1:";
pub const DEDUPE_DELTA_ID_PREFIX_V1: &str = "televy.endpoint_dedupe.delta.v1:";

const DEDUPE_CATALOG_AAD_PREFIX_V1: &str = "televy.endpoint_dedupe.catalog.v1:";

fn scope_for_storage<S: Storage>(storage: &S) -> String {
    storage
        .object_id_scope()
        .unwrap_or_else(|| storage.provider())
        .to_string()
}

pub fn endpoint_dedupe_id_from_scope(scope: &str) -> String {
    format!("{ENDPOINT_DEDUPE_ID_PREFIX_V1}{scope}")
}

pub fn endpoint_dedupe_id_for_storage<S: Storage>(storage: &S) -> Result<String> {
    Ok(endpoint_dedupe_id_from_scope(&scope_for_storage(storage)))
}

pub fn dedupe_base_id_from_scope(scope: &str) -> String {
    format!("{DEDUPE_BASE_ID_PREFIX_V1}{scope}")
}

pub fn dedupe_base_id_for_storage<S: Storage>(storage: &S) -> String {
    dedupe_base_id_from_scope(&scope_for_storage(storage))
}

pub fn dedupe_delta_id_from_scope(scope: &str, uuid_simple: &str) -> String {
    format!("{DEDUPE_DELTA_ID_PREFIX_V1}{scope}:{uuid_simple}")
}

pub fn dedupe_catalog_aad_from_scope(scope: &str) -> Vec<u8> {
    format!("{DEDUPE_CATALOG_AAD_PREFIX_V1}{scope}").into_bytes()
}

pub fn dedupe_catalog_aad_for_storage<S: Storage>(storage: &S) -> Vec<u8> {
    dedupe_catalog_aad_from_scope(&scope_for_storage(storage))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DedupeCatalogV1 {
    pub version: u32,
    pub updated_at: String,
    pub endpoint_dedupe_id: String,
    pub base: DedupeCatalogBase,
    #[serde(default)]
    pub deltas: Vec<DedupeCatalogDelta>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DedupeCatalogBase {
    pub base_id: String,
    pub manifest_object_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DedupeCatalogDelta {
    pub delta_id: String,
    pub manifest_object_id: String,
    pub created_at: String,
    #[serde(default)]
    pub bytes: Option<u64>,
}

pub fn encrypt_dedupe_catalog(
    master_key: &[u8; 32],
    aad: &[u8],
    catalog: &DedupeCatalogV1,
) -> Result<Vec<u8>> {
    if catalog.version != DEDUPE_CATALOG_VERSION {
        return Err(Error::InvalidConfig {
            message: "invalid dedupe catalog version".to_string(),
        });
    }
    let json = serde_json::to_vec(catalog).map_err(|_| Error::InvalidConfig {
        message: "dedupe catalog json encode failed".to_string(),
    })?;
    encrypt_framed(master_key, aad, &json)
}

pub fn decrypt_dedupe_catalog(
    master_key: &[u8; 32],
    aad: &[u8],
    framed: &[u8],
) -> Result<DedupeCatalogV1> {
    let json = decrypt_framed(master_key, aad, framed)?;
    let cat: DedupeCatalogV1 = serde_json::from_slice(&json).map_err(|e| Error::Crypto {
        message: format!("dedupe catalog json decode failed: {e}"),
    })?;
    if cat.version != DEDUPE_CATALOG_VERSION {
        return Err(Error::InvalidConfig {
            message: "dedupe catalog version mismatch".to_string(),
        });
    }
    Ok(cat)
}

pub async fn load_remote_dedupe_catalog<S: Storage>(
    storage: &S,
    master_key: &[u8; 32],
    catalog_object_id: &str,
) -> Result<DedupeCatalogV1> {
    let bytes = storage.download_document(catalog_object_id).await?;
    let aad = dedupe_catalog_aad_for_storage(storage);
    decrypt_dedupe_catalog(master_key, &aad, &bytes).map_err(|e| Error::Crypto {
        message: format!(
            "dedupe catalog decrypt failed: object_id={catalog_object_id}; {e} (check TBK1 master key)"
        ),
    })
}

pub async fn save_remote_dedupe_catalog<S: Storage>(
    storage: &S,
    master_key: &[u8; 32],
    catalog: &DedupeCatalogV1,
) -> Result<String> {
    let aad = dedupe_catalog_aad_for_storage(storage);
    let bytes = encrypt_dedupe_catalog(master_key, &aad, catalog)?;
    storage
        .upload_document("televybackup-dedupe.catalog", bytes)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct ScopedMemStorage {
        inner: Arc<crate::InMemoryStorage>,
        scope: String,
    }

    impl ScopedMemStorage {
        fn new(inner: Arc<crate::InMemoryStorage>, scope: &str) -> Self {
            Self {
                inner,
                scope: scope.to_string(),
            }
        }
    }

    impl Storage for ScopedMemStorage {
        fn provider(&self) -> &str {
            "test.scoped"
        }

        fn object_id_scope(&self) -> Option<&str> {
            Some(self.scope.as_str())
        }

        fn upload_document<'a>(
            &'a self,
            filename: &'a str,
            bytes: Vec<u8>,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<String>> + Send + 'a>>
        {
            let inner = Arc::clone(&self.inner);
            Box::pin(async move { inner.upload_document(filename, bytes).await })
        }

        fn download_document<'a>(
            &'a self,
            object_id: &'a str,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<Vec<u8>>> + Send + 'a>>
        {
            let inner = Arc::clone(&self.inner);
            Box::pin(async move { inner.download_document(object_id).await })
        }
    }

    #[tokio::test]
    async fn catalog_round_trip_and_aad_is_scoped() {
        let key = [9u8; 32];
        let inner = Arc::new(crate::InMemoryStorage::new());
        let store_a = ScopedMemStorage::new(Arc::clone(&inner), "scope_a");
        let store_b = ScopedMemStorage::new(inner, "scope_b");

        let endpoint_dedupe_id = endpoint_dedupe_id_for_storage(&store_a).unwrap();
        let cat = DedupeCatalogV1 {
            version: DEDUPE_CATALOG_VERSION,
            updated_at: "2026-03-02T00:00:00Z".to_string(),
            endpoint_dedupe_id: endpoint_dedupe_id.clone(),
            base: DedupeCatalogBase {
                base_id: dedupe_base_id_for_storage(&store_a),
                manifest_object_id: "obj_base".to_string(),
            },
            deltas: vec![DedupeCatalogDelta {
                delta_id: dedupe_delta_id_from_scope("scope_a", "u"),
                manifest_object_id: "obj_delta".to_string(),
                created_at: "2026-03-02T00:00:01Z".to_string(),
                bytes: Some(123),
            }],
        };

        let object_id = save_remote_dedupe_catalog(&store_a, &key, &cat)
            .await
            .unwrap();
        let loaded = load_remote_dedupe_catalog(&store_a, &key, &object_id)
            .await
            .unwrap();
        assert_eq!(loaded.endpoint_dedupe_id, endpoint_dedupe_id);

        // Wrong scope must fail decryption.
        let err = load_remote_dedupe_catalog(&store_b, &key, &object_id)
            .await
            .unwrap_err();
        match err {
            Error::Crypto { .. } => {}
            other => panic!("unexpected error: {other:?}"),
        }
    }
}
