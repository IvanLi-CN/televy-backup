use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use base64::Engine;
use serde::{Deserialize, Serialize};

use super::Storage;
use crate::{Error, Result};

const TG_MTPROTO_OBJECT_ID_PREFIX_V1: &str = "tgmtproto:v1:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TgMtProtoObjectIdV1 {
    pub peer: String,
    pub msg_id: i32,
    pub doc_id: i64,
    pub access_hash: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TgMtProtoObjectIdV1Payload {
    peer: String,
    #[serde(rename = "msgId")]
    msg_id: String,
    #[serde(rename = "docId")]
    doc_id: String,
    #[serde(rename = "accessHash")]
    access_hash: String,
}

pub fn encode_tgmtproto_object_id_v1(
    peer: &str,
    msg_id: i32,
    doc_id: i64,
    access_hash: i64,
) -> Result<String> {
    let payload = TgMtProtoObjectIdV1Payload {
        peer: peer.to_string(),
        msg_id: msg_id.to_string(),
        doc_id: doc_id.to_string(),
        access_hash: access_hash.to_string(),
    };
    let json = serde_json::to_vec(&payload).map_err(|e| Error::InvalidConfig {
        message: format!("tgmtproto object_id payload json failed: {e}"),
    })?;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
    Ok(format!("{TG_MTPROTO_OBJECT_ID_PREFIX_V1}{b64}"))
}

pub fn parse_tgmtproto_object_id_v1(encoded: &str) -> Result<TgMtProtoObjectIdV1> {
    let b64 = encoded
        .strip_prefix(TG_MTPROTO_OBJECT_ID_PREFIX_V1)
        .ok_or_else(|| Error::Integrity {
            message: format!(
                "invalid tgmtproto object_id (missing {TG_MTPROTO_OBJECT_ID_PREFIX_V1})"
            ),
        })?;

    if b64.contains('+') || b64.contains('@') {
        return Err(Error::Integrity {
            message: "invalid tgmtproto object_id (contains '+' or '@')".to_string(),
        });
    }

    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64.as_bytes())
        .map_err(|e| Error::Integrity {
            message: format!("invalid tgmtproto object_id (bad base64url): {e}"),
        })?;

    let payload: TgMtProtoObjectIdV1Payload =
        serde_json::from_slice(&json).map_err(|e| Error::Integrity {
            message: format!("invalid tgmtproto object_id (bad json): {e}"),
        })?;

    if payload.peer.is_empty() {
        return Err(Error::Integrity {
            message: "invalid tgmtproto object_id (empty peer)".to_string(),
        });
    }

    let msg_id = payload
        .msg_id
        .parse::<i32>()
        .map_err(|_| Error::Integrity {
            message: "invalid tgmtproto object_id (bad msgId)".to_string(),
        })?;
    let doc_id = payload
        .doc_id
        .parse::<i64>()
        .map_err(|_| Error::Integrity {
            message: "invalid tgmtproto object_id (bad docId)".to_string(),
        })?;
    let access_hash = payload
        .access_hash
        .parse::<i64>()
        .map_err(|_| Error::Integrity {
            message: "invalid tgmtproto object_id (bad accessHash)".to_string(),
        })?;

    Ok(TgMtProtoObjectIdV1 {
        peer: payload.peer,
        msg_id,
        doc_id,
        access_hash,
    })
}

#[derive(Debug, Clone)]
pub struct TelegramMtProtoStorageConfig {
    pub provider: String,
    pub api_id: i32,
    pub api_hash: String,
    pub bot_token: String,
    pub chat_id: String,
    pub session: Option<Vec<u8>>,
    pub cache_dir: PathBuf,
    pub helper_path: Option<PathBuf>,
}

pub struct TelegramMtProtoStorage {
    provider: String,
    chat_id: String,
    session: Mutex<Option<Vec<u8>>>,
    helper: Mutex<MtProtoHelper>,
}

impl TelegramMtProtoStorage {
    pub async fn connect(config: TelegramMtProtoStorageConfig) -> Result<Self> {
        let helper_path = config.helper_path.unwrap_or_else(|| {
            default_helper_path().unwrap_or_else(|| PathBuf::from("televybackup-mtproto-helper"))
        });

        let mut helper = MtProtoHelper::spawn(&helper_path)?;
        helper.init(InitRequest {
            api_id: config.api_id,
            api_hash: config.api_hash,
            bot_token: config.bot_token,
            chat_id: config.chat_id.clone(),
            session_b64: config
                .session
                .map(|b| base64::engine::general_purpose::STANDARD.encode(b)),
            cache_dir: config.cache_dir,
        })?;

        Ok(Self {
            provider: config.provider,
            chat_id: config.chat_id,
            session: Mutex::new(helper.session_bytes()),
            helper: Mutex::new(helper),
        })
    }

    pub fn session_bytes(&self) -> Option<Vec<u8>> {
        self.session.lock().ok().and_then(|guard| guard.clone())
    }

    pub fn pinned_object_id(&self) -> Result<Option<String>> {
        let mut helper = self.helper.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper lock poisoned".to_string(),
        })?;
        let out = helper.get_pinned();
        *self.session.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper session lock poisoned".to_string(),
        })? = helper.session_bytes();
        out
    }

    pub fn pin_message_id(&self, msg_id: i32) -> Result<()> {
        let mut helper = self.helper.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper lock poisoned".to_string(),
        })?;
        let out = helper.pin(msg_id);
        *self.session.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper session lock poisoned".to_string(),
        })? = helper.session_bytes();
        out?;
        Ok(())
    }

    pub fn list_dialogs(
        &self,
        limit: usize,
        include_users: bool,
    ) -> Result<Vec<TelegramDialogInfo>> {
        let mut helper = self.helper.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper lock poisoned".to_string(),
        })?;
        let out = helper.list_dialogs(limit, include_users);
        *self.session.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper session lock poisoned".to_string(),
        })? = helper.session_bytes();
        out
    }

    pub fn wait_for_chat(
        &self,
        timeout_secs: u64,
        include_users: bool,
    ) -> Result<TelegramDialogInfo> {
        let mut helper = self.helper.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper lock poisoned".to_string(),
        })?;
        let out = helper.wait_for_chat(timeout_secs, include_users);
        *self.session.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper session lock poisoned".to_string(),
        })? = helper.session_bytes();
        out
    }
}

impl crate::bootstrap::PinnedStorage for TelegramMtProtoStorage {
    fn get_pinned_object_id(&self) -> Result<Option<String>> {
        self.pinned_object_id()
    }

    fn set_pinned_object_id(&self, object_id: &str) -> Result<()> {
        let parsed = parse_tgmtproto_object_id_v1(object_id)?;
        if parsed.peer != self.chat_id {
            return Err(Error::InvalidConfig {
                message: format!(
                    "tgmtproto peer mismatch: expected={} got={}",
                    self.chat_id, parsed.peer
                ),
            });
        }
        self.pin_message_id(parsed.msg_id)
    }
}

impl Storage for TelegramMtProtoStorage {
    fn provider(&self) -> &str {
        &self.provider
    }

    fn object_id_scope(&self) -> Option<&str> {
        Some(&self.chat_id)
    }

    fn upload_document<'a>(
        &'a self,
        filename: &'a str,
        bytes: Vec<u8>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let mut helper = self.helper.lock().map_err(|_| Error::Telegram {
                message: "mtproto helper lock poisoned".to_string(),
            })?;
            let resp = helper.upload(UploadRequest {
                filename: filename.to_string(),
                bytes,
            })?;
            *self.session.lock().map_err(|_| Error::Telegram {
                message: "mtproto helper session lock poisoned".to_string(),
            })? = resp.session;
            Ok(resp.object_id)
        })
    }

    fn upload_document_with_progress<'a>(
        &'a self,
        filename: &'a str,
        bytes: Vec<u8>,
        mut progress: Option<Box<dyn FnMut(u64) + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            let mut helper = self.helper.lock().map_err(|_| Error::Telegram {
                message: "mtproto helper lock poisoned".to_string(),
            })?;
            let resp = {
                let progress = progress.as_deref_mut().map(|cb| cb as &mut dyn FnMut(u64));
                helper.upload_with_progress(
                    UploadRequest {
                        filename: filename.to_string(),
                        bytes,
                    },
                    progress,
                )?
            };
            *self.session.lock().map_err(|_| Error::Telegram {
                message: "mtproto helper session lock poisoned".to_string(),
            })? = resp.session;
            Ok(resp.object_id)
        })
    }

    fn download_document<'a>(
        &'a self,
        object_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>>> + Send + 'a>> {
        Box::pin(async move {
            let parsed = parse_tgmtproto_object_id_v1(object_id)?;
            if parsed.peer != self.chat_id {
                return Err(Error::InvalidConfig {
                    message: format!(
                        "tgmtproto peer mismatch: expected={} got={}",
                        self.chat_id, parsed.peer
                    ),
                });
            }

            let mut helper = self.helper.lock().map_err(|_| Error::Telegram {
                message: "mtproto helper lock poisoned".to_string(),
            })?;
            let resp = helper.download(DownloadRequest {
                object_id: object_id.to_string(),
            })?;
            *self.session.lock().map_err(|_| Error::Telegram {
                message: "mtproto helper session lock poisoned".to_string(),
            })? = resp.session;
            Ok(resp.bytes)
        })
    }
}

fn default_helper_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let sibling = exe.with_file_name("televybackup-mtproto-helper");
    if sibling.exists() {
        return Some(sibling);
    }

    // Dev ergonomics: the helper is built from an excluded crate (`crates/mtproto-helper`), so it
    // won't land next to `target/{debug,release}/televybackup` unless manually copied. If we're
    // running from that typical Cargo layout, try the helper's own target dir.
    //
    // Note: the app bundle path is handled by the sibling check above.
    let parent = exe.parent()?;
    let profile_dir = parent.file_name()?.to_string_lossy();
    if profile_dir != "debug" && profile_dir != "release" {
        return None;
    }

    let target_dir = parent.parent()?;
    if target_dir.file_name()?.to_string_lossy() != "target" {
        return None;
    }

    let root_dir = target_dir.parent()?;
    let candidate = root_dir
        .join("crates")
        .join("mtproto-helper")
        .join("target")
        .join(profile_dir.as_ref())
        .join("televybackup-mtproto-helper");
    if candidate.exists() {
        Some(candidate)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tgmtproto_object_id_v1_roundtrip() {
        let encoded = encode_tgmtproto_object_id_v1("peer", 42, 123456789, 987654321).unwrap();
        assert!(encoded.starts_with(TG_MTPROTO_OBJECT_ID_PREFIX_V1));
        assert!(!encoded.contains('+'));
        assert!(!encoded.contains('@'));
        assert!(!encoded.contains('='));

        let parsed = parse_tgmtproto_object_id_v1(&encoded).unwrap();
        assert_eq!(
            parsed,
            TgMtProtoObjectIdV1 {
                peer: "peer".to_string(),
                msg_id: 42,
                doc_id: 123456789,
                access_hash: 987654321,
            }
        );
    }

    #[test]
    fn tgmtproto_object_id_v1_rejects_pack_delimiters() {
        let bad_plus = format!("{TG_MTPROTO_OBJECT_ID_PREFIX_V1}abc+def");
        assert!(parse_tgmtproto_object_id_v1(&bad_plus).is_err());

        let bad_at = format!("{TG_MTPROTO_OBJECT_ID_PREFIX_V1}abc@def");
        assert!(parse_tgmtproto_object_id_v1(&bad_at).is_err());
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    Init(InitRequest),
    Upload(UploadRequestMeta),
    Download(DownloadRequest),
    GetPinned,
    Pin(PinRequest),
    ListDialogs(ListDialogsRequest),
    WaitForChat(WaitForChatRequest),
}

#[derive(Debug, Serialize)]
struct InitRequest {
    #[serde(rename = "apiId")]
    api_id: i32,
    #[serde(rename = "apiHash")]
    api_hash: String,
    #[serde(rename = "botToken")]
    bot_token: String,
    #[serde(rename = "chatId")]
    chat_id: String,
    #[serde(rename = "session")]
    session_b64: Option<String>,
    #[serde(rename = "cacheDir")]
    cache_dir: PathBuf,
}

#[derive(Debug)]
struct UploadRequest {
    filename: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Serialize)]
struct UploadRequestMeta {
    filename: String,
    size: usize,
}

#[derive(Debug, Serialize)]
struct DownloadRequest {
    #[serde(rename = "objectId")]
    object_id: String,
}

#[derive(Debug, Serialize)]
struct PinRequest {
    #[serde(rename = "msgId")]
    msg_id: i32,
}

#[derive(Debug, Serialize)]
struct ListDialogsRequest {
    limit: usize,
    #[serde(rename = "includeUsers")]
    include_users: bool,
}

#[derive(Debug, Serialize)]
struct WaitForChatRequest {
    #[serde(rename = "timeoutSecs")]
    timeout_secs: u64,
    #[serde(rename = "includeUsers")]
    include_users: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
struct ResponseEnvelope {
    ok: bool,
    error: Option<String>,
    #[serde(rename = "session")]
    session_b64: Option<String>,
    #[serde(flatten)]
    data: serde_json::Value,
}

struct MtProtoHelper {
    _child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    session_b64: Option<String>,
}

struct UploadResponse {
    object_id: String,
    session: Option<Vec<u8>>,
}

struct DownloadResponse {
    bytes: Vec<u8>,
    session: Option<Vec<u8>>,
}

#[derive(Debug, Clone)]
pub struct TelegramDialogInfo {
    pub kind: String,
    pub title: String,
    pub username: Option<String>,
    pub peer_id: i64,
    pub config_chat_id: String,
    pub bootstrap_hint: bool,
}

impl MtProtoHelper {
    fn spawn(path: &Path) -> Result<Self> {
        let mut child = Command::new(path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| Error::InvalidConfig {
                message: format!(
                    "failed to start mtproto helper: {} (path={})",
                    e,
                    path.display()
                ),
            })?;

        let stdin = child.stdin.take().ok_or_else(|| Error::InvalidConfig {
            message: "mtproto helper missing stdin".to_string(),
        })?;
        let stdout = child.stdout.take().ok_or_else(|| Error::InvalidConfig {
            message: "mtproto helper missing stdout".to_string(),
        })?;

        Ok(Self {
            _child: child,
            stdin,
            stdout: BufReader::new(stdout),
            session_b64: None,
        })
    }

    fn session_bytes(&self) -> Option<Vec<u8>> {
        self.session_b64
            .as_ref()
            .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
    }

    fn init(&mut self, req: InitRequest) -> Result<()> {
        self.send_json(&Request::Init(req))?;
        let env = self.read_json_line()?;
        self.apply_session(&env)?;
        if !env.ok {
            return Err(Error::InvalidConfig {
                message: env
                    .error
                    .unwrap_or_else(|| "mtproto init failed".to_string()),
            });
        }
        Ok(())
    }

    fn upload(&mut self, req: UploadRequest) -> Result<UploadResponse> {
        self.upload_with_progress(req, None)
    }

    fn upload_with_progress(
        &mut self,
        req: UploadRequest,
        mut on_progress: Option<&mut dyn FnMut(u64)>,
    ) -> Result<UploadResponse> {
        let meta = UploadRequestMeta {
            filename: req.filename,
            size: req.bytes.len(),
        };
        self.send_json(&Request::Upload(meta))?;
        self.stdin
            .write_all(&req.bytes)
            .map_err(|e| Error::Telegram {
                message: format!("mtproto helper upload write failed: {e}"),
            })?;
        self.stdin.flush().ok();

        loop {
            let env = self.read_json_line()?;
            self.apply_session(&env)?;
            if !env.ok {
                return Err(Error::Telegram {
                    message: env
                        .error
                        .unwrap_or_else(|| "mtproto upload failed".to_string()),
                });
            }

            let event = env
                .data
                .get("event")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if event == "upload_progress" {
                if let (Some(bytes), Some(cb)) = (
                    env.data.get("bytesUploaded").and_then(|v| v.as_u64()),
                    on_progress.as_mut(),
                ) {
                    (**cb)(bytes);
                }
                continue;
            }

            let object_id = env
                .data
                .get("objectId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Telegram {
                    message: "mtproto upload missing objectId".to_string(),
                })?
                .to_string();

            return Ok(UploadResponse {
                object_id,
                session: self.session_bytes(),
            });
        }
    }

    fn download(&mut self, req: DownloadRequest) -> Result<DownloadResponse> {
        self.send_json(&Request::Download(req))?;

        let env = self.read_json_line()?;
        self.apply_session(&env)?;
        if !env.ok {
            return Err(Error::Telegram {
                message: env
                    .error
                    .unwrap_or_else(|| "mtproto download failed".to_string()),
            });
        }

        let size = env
            .data
            .get("size")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| Error::Telegram {
                message: "mtproto download missing size".to_string(),
            })?;
        if size > (usize::MAX as u64) {
            return Err(Error::InvalidConfig {
                message: "mtproto download too large".to_string(),
            });
        }

        let mut bytes = vec![0u8; size as usize];
        self.stdout
            .read_exact(&mut bytes)
            .map_err(|e| Error::Telegram {
                message: format!("mtproto download read failed: {e}"),
            })?;

        Ok(DownloadResponse {
            bytes,
            session: self.session_bytes(),
        })
    }

    fn get_pinned(&mut self) -> Result<Option<String>> {
        self.send_json(&Request::GetPinned)?;

        let env = self.read_json_line()?;
        self.apply_session(&env)?;
        if !env.ok {
            return Err(Error::Telegram {
                message: env
                    .error
                    .unwrap_or_else(|| "mtproto get_pinned failed".to_string()),
            });
        }

        let v = env.data.get("objectId").ok_or_else(|| Error::Telegram {
            message: "mtproto get_pinned missing objectId".to_string(),
        })?;
        if v.is_null() {
            return Ok(None);
        }
        let object_id = v.as_str().ok_or_else(|| Error::Telegram {
            message: "mtproto get_pinned invalid objectId".to_string(),
        })?;
        Ok(Some(object_id.to_string()))
    }

    fn pin(&mut self, msg_id: i32) -> Result<()> {
        self.send_json(&Request::Pin(PinRequest { msg_id }))?;

        let env = self.read_json_line()?;
        self.apply_session(&env)?;
        if !env.ok {
            return Err(Error::Telegram {
                message: env
                    .error
                    .unwrap_or_else(|| "mtproto pin failed".to_string()),
            });
        }
        Ok(())
    }

    fn list_dialogs(
        &mut self,
        limit: usize,
        include_users: bool,
    ) -> Result<Vec<TelegramDialogInfo>> {
        self.send_json(&Request::ListDialogs(ListDialogsRequest {
            limit,
            include_users,
        }))?;

        let env = self.read_json_line()?;
        self.apply_session(&env)?;
        if !env.ok {
            return Err(Error::Telegram {
                message: env
                    .error
                    .unwrap_or_else(|| "mtproto list_dialogs failed".to_string()),
            });
        }

        let dialogs = env
            .data
            .get("dialogs")
            .and_then(|v| v.as_array())
            .ok_or_else(|| Error::Telegram {
                message: "mtproto list_dialogs missing dialogs".to_string(),
            })?;

        let mut out = Vec::with_capacity(dialogs.len());
        for d in dialogs {
            let kind = d
                .get("kind")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Telegram {
                    message: "mtproto list_dialogs invalid kind".to_string(),
                })?
                .to_string();
            let title = d
                .get("title")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Telegram {
                    message: "mtproto list_dialogs invalid title".to_string(),
                })?
                .to_string();
            let username = d
                .get("username")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let peer_id =
                d.get("peerId")
                    .and_then(|v| v.as_i64())
                    .ok_or_else(|| Error::Telegram {
                        message: "mtproto list_dialogs invalid peerId".to_string(),
                    })?;
            let config_chat_id = d
                .get("configChatId")
                .and_then(|v| v.as_str())
                .ok_or_else(|| Error::Telegram {
                    message: "mtproto list_dialogs invalid configChatId".to_string(),
                })?
                .to_string();
            let bootstrap_hint = d
                .get("bootstrapHint")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            out.push(TelegramDialogInfo {
                kind,
                title,
                username,
                peer_id,
                config_chat_id,
                bootstrap_hint,
            });
        }

        Ok(out)
    }

    fn wait_for_chat(
        &mut self,
        timeout_secs: u64,
        include_users: bool,
    ) -> Result<TelegramDialogInfo> {
        self.send_json(&Request::WaitForChat(WaitForChatRequest {
            timeout_secs,
            include_users,
        }))?;

        let env = self.read_json_line()?;
        self.apply_session(&env)?;
        if !env.ok {
            return Err(Error::Telegram {
                message: env
                    .error
                    .unwrap_or_else(|| "mtproto wait_for_chat failed".to_string()),
            });
        }

        let d = env.data.get("chat").ok_or_else(|| Error::Telegram {
            message: "mtproto wait_for_chat missing chat".to_string(),
        })?;

        let kind = d
            .get("kind")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Telegram {
                message: "mtproto wait_for_chat invalid kind".to_string(),
            })?
            .to_string();
        let title = d
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Telegram {
                message: "mtproto wait_for_chat invalid title".to_string(),
            })?
            .to_string();
        let username = d
            .get("username")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let peer_id = d
            .get("peerId")
            .and_then(|v| v.as_i64())
            .ok_or_else(|| Error::Telegram {
                message: "mtproto wait_for_chat invalid peerId".to_string(),
            })?;
        let config_chat_id = d
            .get("configChatId")
            .and_then(|v| v.as_str())
            .ok_or_else(|| Error::Telegram {
                message: "mtproto wait_for_chat invalid configChatId".to_string(),
            })?
            .to_string();
        let bootstrap_hint = d
            .get("bootstrapHint")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(TelegramDialogInfo {
            kind,
            title,
            username,
            peer_id,
            config_chat_id,
            bootstrap_hint,
        })
    }

    fn apply_session(&mut self, env: &ResponseEnvelope) -> Result<()> {
        if let Some(b64) = &env.session_b64
            && !b64.is_empty()
        {
            self.session_b64 = Some(b64.to_string());
        }
        Ok(())
    }

    fn send_json(&mut self, req: &Request) -> Result<()> {
        let line = serde_json::to_string(req).map_err(|e| Error::InvalidConfig {
            message: format!("mtproto helper request json failed: {e}"),
        })?;
        self.stdin
            .write_all(line.as_bytes())
            .map_err(|e| Error::Telegram {
                message: format!("mtproto helper write failed: {e}"),
            })?;
        self.stdin.write_all(b"\n").map_err(|e| Error::Telegram {
            message: format!("mtproto helper write failed: {e}"),
        })?;
        self.stdin.flush().ok();
        Ok(())
    }

    fn read_json_line(&mut self) -> Result<ResponseEnvelope> {
        let mut line = String::new();
        let n = self
            .stdout
            .read_line(&mut line)
            .map_err(|e| Error::Telegram {
                message: format!("mtproto helper read failed: {e}"),
            })?;
        if n == 0 {
            return Err(Error::Telegram {
                message: "mtproto helper closed stdout".to_string(),
            });
        }
        serde_json::from_str::<ResponseEnvelope>(line.trim_end()).map_err(|e| Error::Telegram {
            message: format!("mtproto helper invalid response: {e}"),
        })
    }
}
