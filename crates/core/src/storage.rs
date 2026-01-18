use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::Deserialize;
use tokio::sync::Mutex;

use crate::{Error, Result};

pub trait Storage {
    fn provider(&self) -> &'static str;

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

#[derive(Debug, Clone)]
pub struct TelegramBotApiStorageConfig {
    pub bot_token: String,
    pub chat_id: String,
}

pub struct TelegramBotApiStorage {
    config: TelegramBotApiStorageConfig,
    client: reqwest::Client,
}

impl TelegramBotApiStorage {
    pub fn new(config: TelegramBotApiStorageConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
        }
    }
}

impl Storage for TelegramBotApiStorage {
    fn provider(&self) -> &'static str {
        "telegram.botapi"
    }

    fn upload_document<'a>(
        &'a self,
        filename: &'a str,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!(
                "https://api.telegram.org/bot{}/sendDocument",
                self.config.bot_token
            );

            let part = reqwest::multipart::Part::bytes(bytes).file_name(filename.to_string());
            let form = reqwest::multipart::Form::new()
                .text("chat_id", self.config.chat_id.clone())
                .part("document", part);

            let res = self
                .client
                .post(url)
                .multipart(form)
                .send()
                .await
                .map_err(|e| Error::Telegram {
                    message: format!("request failed: {e}"),
                })?;

            let status = res.status();
            let body = res.text().await.map_err(|e| Error::Telegram {
                message: format!("read response failed: {e}"),
            })?;

            if !status.is_success() {
                return Err(Error::Telegram {
                    message: format!("http {status}: {body}"),
                });
            }

            let parsed: TelegramResponse<TelegramMessage> =
                serde_json::from_str(&body).map_err(|e| Error::Telegram {
                    message: format!("invalid json: {e}; body={body}"),
                })?;

            if !parsed.ok {
                return Err(Error::Telegram {
                    message: parsed
                        .description
                        .unwrap_or_else(|| "telegram returned ok=false".to_string()),
                });
            }

            let file_id = parsed
                .result
                .document
                .ok_or_else(|| Error::Telegram {
                    message: "missing result.document".to_string(),
                })?
                .file_id;
            Ok(file_id)
        })
    }

    fn download_document<'a>(
        &'a self,
        object_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let url = format!(
                "https://api.telegram.org/bot{}/getFile",
                self.config.bot_token
            );
            let res = self
                .client
                .get(url)
                .query(&[("file_id", object_id)])
                .send()
                .await
                .map_err(|e| Error::Telegram {
                    message: format!("getFile request failed: {e}"),
                })?;

            let status = res.status();
            let body = res.text().await.map_err(|e| Error::Telegram {
                message: format!("getFile read response failed: {e}"),
            })?;

            if !status.is_success() {
                return Err(Error::Telegram {
                    message: format!("getFile http {status}: {body}"),
                });
            }

            let parsed: TelegramResponse<TelegramGetFileResult> = serde_json::from_str(&body)
                .map_err(|e| Error::Telegram {
                    message: format!("getFile invalid json: {e}; body={body}"),
                })?;

            if !parsed.ok {
                return Err(Error::Telegram {
                    message: parsed
                        .description
                        .unwrap_or_else(|| "telegram returned ok=false".to_string()),
                });
            }

            let file_path = parsed.result.file_path.ok_or_else(|| Error::Telegram {
                message: "getFile missing result.file_path".to_string(),
            })?;

            let download_url = format!(
                "https://api.telegram.org/file/bot{}/{}",
                self.config.bot_token, file_path
            );
            let res = self
                .client
                .get(download_url)
                .send()
                .await
                .map_err(|e| Error::Telegram {
                    message: format!("file download failed: {e}"),
                })?;

            let status = res.status();
            let bytes = res.bytes().await.map_err(|e| Error::Telegram {
                message: format!("file download read failed: {e}"),
            })?;
            if !status.is_success() {
                return Err(Error::Telegram {
                    message: format!("file download http {status}"),
                });
            }

            Ok(bytes.to_vec())
        })
    }
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
    fn provider(&self) -> &'static str {
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

#[derive(Debug, Deserialize)]
struct TelegramResponse<T> {
    ok: bool,
    result: T,
    description: Option<String>,
}

#[derive(Debug, Deserialize)]
struct TelegramMessage {
    document: Option<TelegramDocument>,
}

#[derive(Debug, Deserialize)]
struct TelegramDocument {
    file_id: String,
}

#[derive(Debug, Deserialize)]
struct TelegramGetFileResult {
    file_path: Option<String>,
}
