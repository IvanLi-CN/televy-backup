use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::sync::Mutex;

use crate::{Error, Result};

pub(crate) const MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES: usize = 128 * 1024 * 1024;

mod telegram_mtproto;
pub use telegram_mtproto::{
    TelegramDialogInfo, TelegramMtProtoStorage, TelegramMtProtoStorageConfig, TgMtProtoObjectIdV1,
    encode_tgmtproto_object_id_v1, parse_tgmtproto_object_id_v1,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChunkObjectRef {
    Direct {
        object_id: String,
    },
    PackSlice {
        pack_object_id: String,
        offset: u64,
        len: u64,
    },
}

pub fn encode_tgfile_object_id(object_id: &str) -> String {
    format!("tgfile:{object_id}")
}

pub fn encode_tgpack_object_id(pack_object_id: &str, offset: u64, len: u64) -> String {
    format!("tgpack:{pack_object_id}@{offset}+{len}")
}

pub fn parse_chunk_object_ref(encoded: &str) -> Result<ChunkObjectRef> {
    if let Some(rest) = encoded.strip_prefix("tgfile:") {
        if rest.is_empty() {
            return Err(Error::Integrity {
                message: "invalid tgfile object_id (empty)".to_string(),
            });
        }
        return Ok(ChunkObjectRef::Direct {
            object_id: rest.to_string(),
        });
    }

    if let Some(rest) = encoded.strip_prefix("tgpack:") {
        let (left, len_str) = rest.rsplit_once('+').ok_or_else(|| Error::Integrity {
            message: "invalid tgpack object_id (missing '+')".to_string(),
        })?;
        let (pack_object_id, offset_str) =
            left.rsplit_once('@').ok_or_else(|| Error::Integrity {
                message: "invalid tgpack object_id (missing '@')".to_string(),
            })?;

        if pack_object_id.is_empty() {
            return Err(Error::Integrity {
                message: "invalid tgpack object_id (empty pack_object_id)".to_string(),
            });
        }

        let offset = offset_str.parse::<u64>().map_err(|_| Error::Integrity {
            message: "invalid tgpack object_id (bad offset)".to_string(),
        })?;
        let len = len_str.parse::<u64>().map_err(|_| Error::Integrity {
            message: "invalid tgpack object_id (bad len)".to_string(),
        })?;

        return Ok(ChunkObjectRef::PackSlice {
            pack_object_id: pack_object_id.to_string(),
            offset,
            len,
        });
    }

    Ok(ChunkObjectRef::Direct {
        object_id: encoded.to_string(),
    })
}

pub trait Storage {
    fn provider(&self) -> &str;

    /// Optional scope identifier embedded into object IDs (e.g. Telegram peer / chat_id).
    ///
    /// When present, callers may use it to detect stale `chunk_objects` rows that reference a
    /// different remote location (e.g. the user changed `chat_id` from a DM to a channel).
    fn object_id_scope(&self) -> Option<&str> {
        None
    }

    fn upload_document<'a>(
        &'a self,
        filename: &'a str,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>>;

    fn download_document<'a>(
        &'a self,
        object_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>>;
}

#[derive(Debug, Default)]
pub struct InMemoryStorage {
    pub uploaded: AtomicUsize,
    inner: Mutex<HashMap<String, Vec<u8>>>,
}

impl InMemoryStorage {
    pub fn new() -> Self {
        Self::default()
    }

    pub async fn get(&self, object_id: &str) -> Option<Vec<u8>> {
        self.inner.lock().await.get(object_id).cloned()
    }

    pub async fn remove(&self, object_id: &str) -> Option<Vec<u8>> {
        self.inner.lock().await.remove(object_id)
    }

    pub async fn object_count(&self) -> usize {
        self.inner.lock().await.len()
    }
}

impl Storage for InMemoryStorage {
    fn provider(&self) -> &str {
        "test.mem"
    }

    fn upload_document<'a>(
        &'a self,
        _filename: &'a str,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let object_id = format!("mem:{}", uuid::Uuid::new_v4());
            self.inner.lock().await.insert(object_id.clone(), bytes);
            self.uploaded.fetch_add(1, Ordering::Relaxed);
            Ok(object_id)
        })
    }

    fn download_document<'a>(
        &'a self,
        object_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            self.inner
                .lock()
                .await
                .get(object_id)
                .cloned()
                .ok_or_else(|| Error::InvalidConfig {
                    message: format!("object not found: {object_id}"),
                })
        })
    }
}
