use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use grammers_client::client::files::MAX_CHUNK_SIZE;
use grammers_client::grammers_tl_types as tl;
use grammers_client::session::defs::{PeerAuth, PeerId, PeerRef};
use grammers_client::session::storages::TlSession;
use grammers_client::types::media::Uploaded;
use grammers_client::types::{Media, Peer};
use grammers_client::{Client, InputMessage, Update, UpdatesConfiguration};
use grammers_mtsender::SenderPool;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tokio::time::{Duration, timeout};

const TG_MTPROTO_OBJECT_ID_PREFIX_V1: &str = "tgmtproto:v1:";
const INIT_IS_AUTHORIZED_TIMEOUT_SECS: u64 = 120;
const INIT_BOT_SIGN_IN_TIMEOUT_SECS: u64 = 120;
const INIT_RESOLVE_CHAT_TIMEOUT_SECS: u64 = 60;
const UPLOAD_SEND_MESSAGE_TIMEOUT_SECS: u64 = 60;
const DOWNLOAD_GET_MESSAGE_TIMEOUT_SECS: u64 = 60;
const DOWNLOAD_CHUNK_TIMEOUT_SECS: u64 = 60;
const LIST_DIALOGS_TIMEOUT_SECS: u64 = 30;
const WAIT_FOR_CHAT_TIMEOUT_SECS_DEFAULT: u64 = 60;
const WAIT_FOR_CHAT_TIMEOUT_SECS_MAX: u64 = 10 * 60;
const UPLOAD_SAVE_FILE_PART_TIMEOUT_SECS: u64 = 60;
const UPLOAD_SAVE_BIG_FILE_PART_TIMEOUT_SECS: u64 = 60;
const UPLOAD_BIG_FILE_SIZE_BYTES: usize = 10 * 1024 * 1024;
const UPLOAD_WORKER_COUNT: usize = 4;
// Telegram's upload.*Part methods allow up to 512KiB per part. Using a smaller part size makes
// progress/rate updates smoother and reduces the chance of long "stalls" where the network is
// sending but we cannot observe partial progress inside one request.
const UPLOAD_PART_SIZE_BYTES: usize = 128 * 1024;

fn upload_stream_timeout_secs(size: usize) -> u64 {
    // Scale with size to avoid hanging forever, while allowing large objects to complete on slow links.
    let min = 60u64;
    let max = 30 * 60;
    let size = size as u64;

    // ~32KiB/s baseline + fixed overhead.
    let scaled = min.saturating_add(size / (32 * 1024));
    scaled.clamp(min, max)
}

#[derive(Debug, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    Init(InitRequest),
    Upload(UploadRequest),
    Download(DownloadRequest),
    GetPinned,
    Pin(PinRequest),
    ListDialogs(ListDialogsRequest),
    WaitForChat(WaitForChatRequest),
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
struct UploadRequest {
    filename: String,
    size: usize,
}

#[derive(Debug, Deserialize)]
struct DownloadRequest {
    #[serde(rename = "objectId")]
    object_id: String,
}

#[derive(Debug, Deserialize)]
struct PinRequest {
    #[serde(rename = "msgId")]
    msg_id: i32,
}

#[derive(Debug, Deserialize)]
struct ListDialogsRequest {
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default, rename = "includeUsers")]
    include_users: bool,
}

#[derive(Debug, Deserialize)]
struct WaitForChatRequest {
    #[serde(default, rename = "timeoutSecs")]
    timeout_secs: Option<u64>,
    #[serde(default, rename = "includeUsers")]
    include_users: bool,
}

#[derive(Debug, Serialize)]
struct Response {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "session")]
    session_b64: Option<String>,
    #[serde(flatten)]
    data: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct DialogInfo {
    kind: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(rename = "peerId")]
    peer_id: i64,
    #[serde(rename = "configChatId")]
    config_chat_id: String,
    #[serde(rename = "bootstrapHint")]
    bootstrap_hint: bool,
}

struct State {
    chat_id: String,
    cache_dir: PathBuf,
    session: Arc<TlSession>,
    client: Client,
    chat: Option<Peer>,
    updates: grammers_client::client::updates::UpdateStream,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct TgMtProtoObjectIdV1Payload {
    peer: String,
    #[serde(rename = "msgId")]
    msg_id: String,
    #[serde(rename = "docId")]
    doc_id: String,
    #[serde(rename = "accessHash")]
    access_hash: String,
}

#[tokio::main]
async fn main() {
    let stdin = std::io::stdin();
    let mut input = BufReader::new(stdin.lock());
    let stdout = std::io::stdout();
    let mut output = stdout.lock();

    let mut state: Option<State> = None;

    loop {
        let mut line = String::new();
        match input.read_line(&mut line) {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                let _ = write_response(
                    &mut output,
                    Response {
                        ok: false,
                        error: Some(format!("stdin read failed: {e}")),
                        session_b64: None,
                        data: BTreeMap::new(),
                    },
                );
                return;
            }
        }

        let line = line.trim_end();
        if line.is_empty() {
            continue;
        }

        let req: Request = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                let _ = write_response(
                    &mut output,
                    Response {
                        ok: false,
                        error: Some(format!("invalid request json: {e}")),
                        session_b64: None,
                        data: BTreeMap::new(),
                    },
                );
                continue;
            }
        };

        match req {
            Request::Init(req) => {
                let res = init(req).await;
                match res {
                    Ok(s) => {
                        let session_b64 = session_b64(&s.session);
                        state = Some(s);
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: true,
                                error: None,
                                session_b64: Some(session_b64),
                                data: BTreeMap::new(),
                            },
                        );
                    }
                    Err(err) => {
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: false,
                                error: Some(err),
                                session_b64: None,
                                data: BTreeMap::new(),
                            },
                        );
                    }
                }
            }
            Request::Upload(req) => {
                let Some(s) = state.as_mut() else {
                    let _ = write_response(
                        &mut output,
                        Response {
                            ok: false,
                            error: Some("not initialized".to_string()),
                            session_b64: None,
                            data: BTreeMap::new(),
                        },
                    );
                    continue;
                };

                let mut bytes = vec![0u8; req.size];
                if let Err(e) = input.read_exact(&mut bytes) {
                    let _ = write_response(
                        &mut output,
                        Response {
                            ok: false,
                            error: Some(format!("upload bytes read failed: {e}")),
                            session_b64: Some(session_b64(&s.session)),
                            data: BTreeMap::new(),
                        },
                    );
                    continue;
                }

                let res = upload_with_progress(s, req.filename, bytes, &mut output).await;
                match res {
                    Ok(object_id) => {
                        let mut data = BTreeMap::new();
                        data.insert("objectId".to_string(), serde_json::Value::String(object_id));
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: true,
                                error: None,
                                session_b64: Some(session_b64(&s.session)),
                                data,
                            },
                        );
                    }
                    Err(err) => {
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: false,
                                error: Some(err),
                                session_b64: Some(session_b64(&s.session)),
                                data: BTreeMap::new(),
                            },
                        );
                    }
                }
            }
            Request::Download(req) => {
                let Some(s) = state.as_mut() else {
                    let _ = write_response(
                        &mut output,
                        Response {
                            ok: false,
                            error: Some("not initialized".to_string()),
                            session_b64: None,
                            data: BTreeMap::new(),
                        },
                    );
                    continue;
                };

                let res = download_to_cache(s, &req.object_id).await;
                match res {
                    Ok(bytes_path) => {
                        let mut f = match std::fs::File::open(&bytes_path) {
                            Ok(f) => f,
                            Err(e) => {
                                let _ = write_response(
                                    &mut output,
                                    Response {
                                        ok: false,
                                        error: Some(format!("cache file open failed: {e}")),
                                        session_b64: Some(session_b64(&s.session)),
                                        data: BTreeMap::new(),
                                    },
                                );
                                continue;
                            }
                        };

                        let size = match f.metadata() {
                            Ok(m) => m.len(),
                            Err(e) => {
                                let _ = write_response(
                                    &mut output,
                                    Response {
                                        ok: false,
                                        error: Some(format!("cache file stat failed: {e}")),
                                        session_b64: Some(session_b64(&s.session)),
                                        data: BTreeMap::new(),
                                    },
                                );
                                continue;
                            }
                        };

                        let mut data = BTreeMap::new();
                        data.insert("size".to_string(), serde_json::json!(size));
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: true,
                                error: None,
                                session_b64: Some(session_b64(&s.session)),
                                data,
                            },
                        );

                        if let Err(e) = std::io::copy(&mut f, &mut output) {
                            let _ = output.flush();
                            eprintln!("stdout copy failed: {e}");
                            // Do not attempt to write another JSON line: the core side will
                            // already be expecting raw bytes. Hard-exit so stdout closes.
                            std::process::exit(1);
                        }
                        let _ = output.flush();

                        let _ = std::fs::remove_file(&bytes_path);
                    }
                    Err(err) => {
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: false,
                                error: Some(err),
                                session_b64: Some(session_b64(&s.session)),
                                data: BTreeMap::new(),
                            },
                        );
                    }
                }
            }
            Request::GetPinned => {
                let Some(s) = state.as_mut() else {
                    let _ = write_response(
                        &mut output,
                        Response {
                            ok: false,
                            error: Some("not initialized".to_string()),
                            session_b64: None,
                            data: BTreeMap::new(),
                        },
                    );
                    continue;
                };

                let res = get_pinned_object_id(s).await;
                match res {
                    Ok(object_id) => {
                        let mut data = BTreeMap::new();
                        match object_id {
                            Some(id) => {
                                data.insert("objectId".to_string(), serde_json::Value::String(id));
                            }
                            None => {
                                data.insert("objectId".to_string(), serde_json::Value::Null);
                            }
                        }
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: true,
                                error: None,
                                session_b64: Some(session_b64(&s.session)),
                                data,
                            },
                        );
                    }
                    Err(err) => {
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: false,
                                error: Some(err),
                                session_b64: Some(session_b64(&s.session)),
                                data: BTreeMap::new(),
                            },
                        );
                    }
                }
            }
            Request::Pin(req) => {
                let Some(s) = state.as_mut() else {
                    let _ = write_response(
                        &mut output,
                        Response {
                            ok: false,
                            error: Some("not initialized".to_string()),
                            session_b64: None,
                            data: BTreeMap::new(),
                        },
                    );
                    continue;
                };

                let res = pin_message(s, req.msg_id).await;
                match res {
                    Ok(()) => {
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: true,
                                error: None,
                                session_b64: Some(session_b64(&s.session)),
                                data: BTreeMap::new(),
                            },
                        );
                    }
                    Err(err) => {
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: false,
                                error: Some(err),
                                session_b64: Some(session_b64(&s.session)),
                                data: BTreeMap::new(),
                            },
                        );
                    }
                }
            }
            Request::ListDialogs(req) => {
                let Some(s) = state.as_mut() else {
                    let _ = write_response(
                        &mut output,
                        Response {
                            ok: false,
                            error: Some("not initialized".to_string()),
                            session_b64: None,
                            data: BTreeMap::new(),
                        },
                    );
                    continue;
                };

                let limit = req.limit.unwrap_or(200).clamp(1, 5_000);
                let res = list_dialogs(s, limit, req.include_users).await;
                match res {
                    Ok(dialogs) => {
                        let mut data = BTreeMap::new();
                        data.insert("dialogs".to_string(), serde_json::json!(dialogs));
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: true,
                                error: None,
                                session_b64: Some(session_b64(&s.session)),
                                data,
                            },
                        );
                    }
                    Err(err) => {
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: false,
                                error: Some(err),
                                session_b64: Some(session_b64(&s.session)),
                                data: BTreeMap::new(),
                            },
                        );
                    }
                }
            }
            Request::WaitForChat(req) => {
                let Some(s) = state.as_mut() else {
                    let _ = write_response(
                        &mut output,
                        Response {
                            ok: false,
                            error: Some("not initialized".to_string()),
                            session_b64: None,
                            data: BTreeMap::new(),
                        },
                    );
                    continue;
                };

                let timeout_secs = req.timeout_secs.unwrap_or(WAIT_FOR_CHAT_TIMEOUT_SECS_DEFAULT);
                let res = wait_for_chat(s, timeout_secs, req.include_users).await;
                match res {
                    Ok(chat) => {
                        let mut data = BTreeMap::new();
                        data.insert("chat".to_string(), serde_json::json!(chat));
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: true,
                                error: None,
                                session_b64: Some(session_b64(&s.session)),
                                data,
                            },
                        );
                    }
                    Err(err) => {
                        let _ = write_response(
                            &mut output,
                            Response {
                                ok: false,
                                error: Some(err),
                                session_b64: Some(session_b64(&s.session)),
                                data: BTreeMap::new(),
                            },
                        );
                    }
                }
            }
        }
    }
}

fn write_response(out: &mut impl Write, res: Response) -> std::io::Result<()> {
    let line = serde_json::to_string(&res)
        .unwrap_or_else(|_| "{\"ok\":false,\"error\":\"failed to encode response\"}".to_string());
    out.write_all(line.as_bytes())?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn session_b64(session: &TlSession) -> String {
    base64::engine::general_purpose::STANDARD.encode(session.save())
}

async fn init(req: InitRequest) -> Result<State, String> {
    let session = match req.session_b64 {
        Some(b64) if !b64.trim().is_empty() => {
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64.as_bytes())
                .map_err(|e| format!("invalid session base64: {e}"))?;
            TlSession::load(&bytes).map_err(|e| format!("session load failed: {e}"))?
        }
        _ => TlSession::new(),
    };
    let session = Arc::new(session);

    let pool = SenderPool::new(Arc::clone(&session), req.api_id);
    let client = Client::new(&pool);
    let SenderPool { runner, updates, .. } = pool;
    tokio::spawn(runner.run());

    let authorized = timeout(
        Duration::from_secs(INIT_IS_AUTHORIZED_TIMEOUT_SECS),
        client.is_authorized(),
    )
    .await
    .map_err(|_| {
        format!(
            "is_authorized timed out after {INIT_IS_AUTHORIZED_TIMEOUT_SECS}s (check network / MTProto reachability)"
        )
    })?
    .map_err(|e| format!("is_authorized failed: {e}"))?;
    if !authorized {
        timeout(
            Duration::from_secs(INIT_BOT_SIGN_IN_TIMEOUT_SECS),
            client.bot_sign_in(&req.bot_token, &req.api_hash),
        )
        .await
        .map_err(|_| {
            format!(
                "bot_sign_in timed out after {INIT_BOT_SIGN_IN_TIMEOUT_SECS}s (check network / MTProto reachability)"
            )
        })?
        .map_err(|e| format!("bot_sign_in failed: {e}"))?;
    }

    let chat = if req.chat_id.trim().is_empty() {
        None
    } else {
        Some(
            timeout(
                Duration::from_secs(INIT_RESOLVE_CHAT_TIMEOUT_SECS),
                resolve_chat(&client, &req.chat_id),
            )
            .await
            .map_err(|_| {
                format!(
                    "resolve_chat timed out after {INIT_RESOLVE_CHAT_TIMEOUT_SECS}s (check chat_id and bot dialog history)"
                )
            })?
            .map_err(|e| format!("resolve chat failed: {e}"))?,
        )
    };

    std::fs::create_dir_all(&req.cache_dir).map_err(|e| format!("cache dir create failed: {e}"))?;

    // For TelevyBackup, updates are used for the interactive `wait-chat` discovery flow: the user
    // is expected to send a *new* message while listening. Avoid replaying older updates received
    // while offline to prevent returning stale dialogs.
    let updates = client.stream_updates(
        updates,
        UpdatesConfiguration {
            catch_up: false,
            ..Default::default()
        },
    );

    Ok(State {
        chat_id: req.chat_id,
        cache_dir: req.cache_dir,
        session,
        client,
        chat,
        updates,
    })
}

fn require_chat(state: &State) -> Result<&Peer, String> {
    if state.chat_id.trim().is_empty() {
        return Err("telegram.chat_id is empty (required for upload/download/pin; dialogs list can run without chat_id)".to_string());
    }
    state
        .chat
        .as_ref()
        .ok_or_else(|| "chat is not resolved (required for upload/download/pin; re-run with a valid chat_id)".to_string())
}

async fn resolve_chat(client: &Client, chat_id: &str) -> Result<Peer, String> {
    let chat_id = chat_id.trim();
    if chat_id.is_empty() {
        return Err("telegram.chat_id is empty".to_string());
    }

    if let Some(username) = chat_id.strip_prefix('@') {
        return client
            .resolve_username(username)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("chat not found: @{username}"));
    }

    if chat_id.chars().all(|c| c.is_ascii_digit() || c == '-') {
        let dialog_id: i64 = chat_id
            .parse()
            .map_err(|_| format!("invalid telegram.chat_id: {chat_id}"))?;
        // Convert Bot API dialog id to a PeerRef using ambient authority (access_hash=0).
        let peer_ref = if dialog_id > 0 {
            PeerRef {
                id: PeerId::user(dialog_id),
                auth: PeerAuth::default(),
            }
        } else if dialog_id <= -1000000000001 {
            let bare = -dialog_id - 1000000000000;
            PeerRef {
                id: PeerId::channel(bare),
                auth: PeerAuth::default(),
            }
        } else {
            let bare = -dialog_id;
            PeerRef {
                id: PeerId::chat(bare),
                auth: PeerAuth::default(),
            }
        };

        return client
            .resolve_peer(peer_ref)
            .await
            .map_err(|e| format!("resolve_peer failed: {e}"));
    }

    client
        .resolve_username(chat_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("chat not found: {chat_id}"))
}

async fn get_pinned_object_id(state: &mut State) -> Result<Option<String>, String> {
    let chat = require_chat(state)?;
    let pinned = match timeout(
        Duration::from_secs(DOWNLOAD_GET_MESSAGE_TIMEOUT_SECS),
        state.client.get_pinned_message(chat),
    )
    .await
    {
        Err(_) => {
            return Err(format!(
                "get_pinned_message timed out after {DOWNLOAD_GET_MESSAGE_TIMEOUT_SECS}s"
            ));
        }
        Ok(Ok(pinned)) => pinned,
        Ok(Err(e)) => {
            // Some chats simply don't have a pinned message. grammers currently turns this into
            // an RPC error ("MESSAGE_IDS_EMPTY") when fetching the pinned message id, which
            // should be treated as "no catalog yet".
            if e.to_string().contains("MESSAGE_IDS_EMPTY") {
                return Ok(None);
            }
            return Err(format!("get_pinned_message failed: {e}"));
        }
    };

    let Some(msg) = pinned else {
        return Ok(None);
    };

    let msg_id = msg.id();
    let Some(media) = msg.media() else {
        // Pinned message exists but isn't a document (e.g. a text message). Treat as no catalog.
        return Ok(None);
    };

    let (doc_id, access_hash) = match extract_document_id(&media) {
        Ok(v) => v,
        // Common real-world case: the chat already has a pinned message that isn't a document.
        // Treat this as "no bootstrap catalog", so the first run can create+pin ours.
        Err(_) => return Ok(None),
    };
    let object_id = encode_tgmtproto_object_id_v1(&state.chat_id, msg_id, doc_id, access_hash)?;
    Ok(Some(object_id))
}

async fn pin_message(state: &mut State, msg_id: i32) -> Result<(), String> {
    let chat = require_chat(state)?;
    timeout(
        Duration::from_secs(UPLOAD_SEND_MESSAGE_TIMEOUT_SECS),
        state.client.pin_message(chat, msg_id),
    )
    .await
    .map_err(|_| format!("pin_message timed out after {UPLOAD_SEND_MESSAGE_TIMEOUT_SECS}s"))?
    .map_err(|e| format!("pin_message failed: {e}"))?;
    Ok(())
}

async fn list_dialogs(
    state: &mut State,
    limit: usize,
    include_users: bool,
) -> Result<Vec<DialogInfo>, String> {
    let fut = async {
        let mut out = Vec::new();
        let mut dialogs = state.client.iter_dialogs();

        while let Some(dialog) = dialogs
            .next()
            .await
            .map_err(|e| format!("iter_dialogs failed: {e}"))?
        {
            let peer = dialog.peer();
            let peer_id = peer.id().bot_api_dialog_id();
            let title = peer.name().unwrap_or("<unknown>").to_string();
            let username = peer.username().map(|s| s.to_string());

            let info = match peer {
                Peer::User(_) => {
                    if !include_users {
                        continue;
                    }
                    DialogInfo {
                        kind: "user".to_string(),
                        title,
                        username,
                        peer_id,
                        config_chat_id: peer_id.to_string(),
                        bootstrap_hint: false,
                    }
                }
                Peer::Group(_) => {
                    let config_chat_id = username
                        .as_ref()
                        .map(|u| format!("@{u}"))
                        .unwrap_or_else(|| format!("{peer_id}"));
                    DialogInfo {
                        kind: "group".to_string(),
                        title,
                        username,
                        peer_id,
                        config_chat_id,
                        bootstrap_hint: true,
                    }
                }
                Peer::Channel(_) => {
                    let config_chat_id = username
                        .as_ref()
                        .map(|u| format!("@{u}"))
                        .unwrap_or_else(|| format!("{peer_id}"));
                    DialogInfo {
                        kind: "channel".to_string(),
                        title,
                        username,
                        peer_id,
                        config_chat_id,
                        bootstrap_hint: true,
                    }
                }
            };

            out.push(info);
            if out.len() >= limit {
                break;
            }
        }

        Ok(out)
    };

    timeout(Duration::from_secs(LIST_DIALOGS_TIMEOUT_SECS), fut)
        .await
        .map_err(|_| format!("list_dialogs timed out after {LIST_DIALOGS_TIMEOUT_SECS}s"))?
}

async fn wait_for_chat(
    state: &mut State,
    timeout_secs: u64,
    include_users: bool,
) -> Result<DialogInfo, String> {
    let timeout_secs = timeout_secs.clamp(1, WAIT_FOR_CHAT_TIMEOUT_SECS_MAX);

    // Drain any already-buffered updates so we only react to messages that arrive *after* the
    // caller started listening. This avoids returning stale dialogs in long-running helper
    // sessions.
    {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(200);
        let mut drained: u32 = 0;
        while tokio::time::Instant::now() < deadline && drained < 1_000 {
            match timeout(Duration::from_millis(0), state.updates.next()).await {
                Ok(Ok(_)) => drained += 1,
                Ok(Err(e)) => return Err(format!("updates next failed: {e}")),
                Err(_) => break,
            }
        }
    }

    let fut = async {
        loop {
            let update = state
                .updates
                .next()
                .await
                .map_err(|e| format!("updates next failed: {e}"))?;

            match update {
                Update::NewMessage(message) if !message.outgoing() => {
                    let peer_id = message.peer_id().bot_api_dialog_id();
                    let peer = match message.peer() {
                        Ok(p) => p.clone(),
                        Err(_) => continue,
                    };

                    let title = peer.name().unwrap_or("<unknown>").to_string();
                    let username = peer.username().map(|s| s.to_string());

                    let info = match peer {
                        Peer::User(_) => {
                            if !include_users {
                                continue;
                            }
                            DialogInfo {
                                kind: "user".to_string(),
                                title,
                                username,
                                peer_id,
                                config_chat_id: peer_id.to_string(),
                                bootstrap_hint: false,
                            }
                        }
                        Peer::Group(_) => {
                            let config_chat_id = username
                                .as_ref()
                                .map(|u| format!("@{u}"))
                                .unwrap_or_else(|| format!("{peer_id}"));
                            DialogInfo {
                                kind: "group".to_string(),
                                title,
                                username,
                                peer_id,
                                config_chat_id,
                                bootstrap_hint: true,
                            }
                        }
                        Peer::Channel(_) => {
                            let config_chat_id = username
                                .as_ref()
                                .map(|u| format!("@{u}"))
                                .unwrap_or_else(|| format!("{peer_id}"));
                            DialogInfo {
                                kind: "channel".to_string(),
                                title,
                                username,
                                peer_id,
                                config_chat_id,
                                bootstrap_hint: true,
                            }
                        }
                    };

                    return Ok(info);
                }
                _ => continue,
            }
        }
    };

    timeout(Duration::from_secs(timeout_secs), fut)
        .await
        .map_err(|_| format!("wait_for_chat timed out after {timeout_secs}s"))?
}

fn generate_upload_file_id() -> i64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64;
    let mixed = nanos ^ n.rotate_left(13);
    (mixed & 0x7fff_ffff_ffff_ffff) as i64
}

fn write_upload_progress(
    out: &mut impl Write,
    session_b64: Option<String>,
    bytes_uploaded: u64,
    bytes_total: u64,
) -> Result<(), String> {
    let mut data = BTreeMap::new();
    data.insert(
        "event".to_string(),
        serde_json::Value::String("upload_progress".to_string()),
    );
    data.insert("bytesUploaded".to_string(), serde_json::json!(bytes_uploaded));
    data.insert("bytesTotal".to_string(), serde_json::json!(bytes_total));
    write_response(
        out,
        Response {
            ok: true,
            error: None,
            session_b64,
            data,
        },
    )
    .map_err(|e| format!("stdout write failed: {e}"))?;
    Ok(())
}

async fn upload_bytes_with_progress(
    state: &mut State,
    filename: String,
    bytes: Vec<u8>,
    out: &mut impl Write,
) -> Result<Uploaded, String> {
    let size = bytes.len();
    let bytes_total = size as u64;
    if size == 0 {
        return Err("invalid upload: empty stream".to_string());
    }

    let file_id = generate_upload_file_id();
    let name = if filename.is_empty() {
        "a".to_string()
    } else {
        filename
    };

    let chunk_size = (MAX_CHUNK_SIZE as usize).min(UPLOAD_PART_SIZE_BYTES);
    let total_parts = ((size + chunk_size - 1) / chunk_size) as i32;

    if size > UPLOAD_BIG_FILE_SIZE_BYTES {
        let bytes = Arc::new(bytes);
        let next_part = Arc::new(AtomicI32::new(0));
        let uploaded = Arc::new(AtomicU64::new(0));
        let client = state.client.clone();

        let mut join_set = JoinSet::new();
        for _ in 0..UPLOAD_WORKER_COUNT {
            let bytes = Arc::clone(&bytes);
            let next_part = Arc::clone(&next_part);
            let uploaded = Arc::clone(&uploaded);
            let client = client.clone();
            join_set.spawn(async move {
                loop {
                    let part = next_part.fetch_add(1, Ordering::Relaxed);
                    if part >= total_parts {
                        break;
                    }
                    let start = (part as usize).saturating_mul(chunk_size);
                    let end = (start + chunk_size).min(bytes.len());
                    let len = end.saturating_sub(start);
                    if len == 0 {
                        continue;
                    }
                    let chunk = bytes[start..end].to_vec();
                    let len_u64 = len as u64;
                    let ok = match timeout(
                        Duration::from_secs(UPLOAD_SAVE_BIG_FILE_PART_TIMEOUT_SECS),
                        client.invoke(&tl::functions::upload::SaveBigFilePart {
                            file_id,
                            file_part: part,
                            file_total_parts: total_parts,
                            bytes: chunk,
                        }),
                    )
                    .await
                    {
                        Ok(Ok(ok)) => ok,
                        Ok(Err(e)) => {
                            return Err(format!("save_big_file_part failed: {e}"));
                        }
                        Err(_) => {
                            return Err(format!(
                                "save_big_file_part timed out after {UPLOAD_SAVE_BIG_FILE_PART_TIMEOUT_SECS}s"
                            ));
                        }
                    };
                    if !ok {
                        return Err("server failed to store uploaded data".to_string());
                    }

                    // Only count bytes once the server acknowledged storing the part.
                    // Using a smaller part size makes this "close enough" to realtime without
                    // needing socket-level accounting.
                    uploaded.fetch_add(len_u64, Ordering::Relaxed);
                }
                Ok::<(), String>(())
            });
        }

        let mut last_reported = 0u64;
        let mut interval = tokio::time::interval(Duration::from_millis(250));

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    let now = uploaded.load(Ordering::Relaxed).min(bytes_total);
                    if now != last_reported {
                        last_reported = now;
                        let session = Some(session_b64(&state.session));
                        write_upload_progress(out, session, last_reported, bytes_total)?;
                    }
                }
                res = join_set.join_next() => {
                    match res {
                        Some(Ok(Ok(()))) => {}
                        Some(Ok(Err(e))) => {
                            join_set.abort_all();
                            return Err(e);
                        }
                        Some(Err(e)) => {
                            join_set.abort_all();
                            return Err(format!("upload worker join failed: {e}"));
                        }
                        None => break,
                    }
                }
            }
        }

        let final_uploaded = uploaded.load(Ordering::Relaxed).min(bytes_total);
        if final_uploaded != last_reported {
            let session = Some(session_b64(&state.session));
            write_upload_progress(out, session, final_uploaded, bytes_total)?;
        }

        return Ok(Uploaded::from_raw(
            tl::types::InputFileBig {
                id: file_id,
                parts: total_parts,
                name,
            }
            .into(),
        ));
    }

    let mut md5 = md5::Context::new();
    let mut bytes_uploaded = 0u64;
    for (part, chunk) in bytes.chunks(chunk_size).enumerate() {
        md5.consume(chunk);

        let ok = timeout(
            Duration::from_secs(UPLOAD_SAVE_FILE_PART_TIMEOUT_SECS),
            state
                .client
                .invoke(&tl::functions::upload::SaveFilePart {
                    file_id,
                    file_part: part as i32,
                    bytes: chunk.to_vec(),
                }),
        )
        .await
        .map_err(|_| format!("save_file_part timed out after {UPLOAD_SAVE_FILE_PART_TIMEOUT_SECS}s"))?
        .map_err(|e| format!("save_file_part failed: {e}"))?;
        if !ok {
            return Err("server failed to store uploaded data".to_string());
        }

        bytes_uploaded = bytes_uploaded.saturating_add(chunk.len() as u64).min(bytes_total);
        let session = Some(session_b64(&state.session));
        write_upload_progress(out, session, bytes_uploaded, bytes_total)?;
    }

    Ok(Uploaded::from_raw(
        tl::types::InputFile {
            id: file_id,
            parts: total_parts,
            name,
            md5_checksum: format!("{:x}", md5.compute()),
        }
        .into(),
    ))
}

async fn upload_with_progress(
    state: &mut State,
    filename: String,
    bytes: Vec<u8>,
    out: &mut impl Write,
) -> Result<String, String> {
    let chat = require_chat(state)?.clone();
    let size = bytes.len();
    let timeout_secs = upload_stream_timeout_secs(size);
    let uploaded = timeout(
        Duration::from_secs(timeout_secs),
        upload_bytes_with_progress(state, filename, bytes, out),
    )
    .await
    .map_err(|_| format!("upload_stream timed out after {timeout_secs}s"))??;

    let msg = timeout(
        Duration::from_secs(UPLOAD_SEND_MESSAGE_TIMEOUT_SECS),
        state
            .client
            .send_message(&chat, InputMessage::new().text("").file(uploaded)),
    )
    .await
    .map_err(|_| format!("send_message timed out after {UPLOAD_SEND_MESSAGE_TIMEOUT_SECS}s"))?
    .map_err(|e| format!("send_message failed: {e}"))?;

    let msg_id = msg.id();
    let media = msg
        .media()
        .ok_or_else(|| "message has no media".to_string())?;
    let (doc_id, access_hash) = extract_document_id(&media)?;

    let object_id = encode_tgmtproto_object_id_v1(&state.chat_id, msg_id, doc_id, access_hash)?;
    Ok(object_id)
}

async fn download_to_cache(state: &mut State, object_id: &str) -> Result<PathBuf, String> {
    let chat = require_chat(state)?;
    let parsed = parse_tgmtproto_object_id_v1(object_id)?;
    if parsed.peer != state.chat_id {
        return Err(format!(
            "peer mismatch: expected {} got {}",
            state.chat_id, parsed.peer
        ));
    }

    let mut msgs = timeout(
        Duration::from_secs(DOWNLOAD_GET_MESSAGE_TIMEOUT_SECS),
        state
            .client
            .get_messages_by_id(chat, &[parsed.msg_id]),
    )
    .await
    .map_err(|_| {
        format!("get_messages_by_id timed out after {DOWNLOAD_GET_MESSAGE_TIMEOUT_SECS}s")
    })?
    .map_err(|e| format!("get_messages_by_id failed: {e}"))?;
    let msg = msgs
        .pop()
        .flatten()
        .ok_or_else(|| format!("message not found: msg_id={}", parsed.msg_id))?;

    let media = msg
        .media()
        .ok_or_else(|| format!("message has no media: msg_id={}", parsed.msg_id))?;
    let (doc_id, access_hash) = extract_document_id(&media)?;
    if doc_id != parsed.doc_id || access_hash != parsed.access_hash {
        return Err(format!(
            "document mismatch: expected docId={} accessHash={} got docId={} accessHash={}",
            parsed.doc_id, parsed.access_hash, doc_id, access_hash
        ));
    }

    let cache_key = blake3::hash(object_id.as_bytes()).to_hex().to_string();
    let cache_path = state.cache_dir.join(format!("{cache_key}.part"));

    let chunk_size = MAX_CHUNK_SIZE as u64;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(&cache_path)
        .map_err(|e| format!("cache file open failed: {e}"))?;
    let mut len = file
        .metadata()
        .map_err(|e| format!("cache file stat failed: {e}"))?
        .len();
    if chunk_size > 0 && len % chunk_size != 0 {
        let trimmed = len - (len % chunk_size);
        eprintln!(
            "mtproto: download cache not aligned; truncating (cache_key={cache_key} from={len} to={trimmed})"
        );
        file.set_len(trimmed)
            .map_err(|e| format!("cache file truncate failed: {e}"))?;
        len = trimmed;
    }

    file.seek(SeekFrom::End(0))
        .map_err(|e| format!("cache file seek failed: {e}"))?;

    if len > 0 {
        eprintln!(
            "mtproto: resuming download from cache (cache_key={cache_key} cached_bytes={len})"
        );
    }

    let mut download = state
        .client
        .iter_download(&media)
        .chunk_size(MAX_CHUNK_SIZE)
        .skip_chunks((len / chunk_size) as i32);

    while let Some(chunk) = timeout(
        Duration::from_secs(DOWNLOAD_CHUNK_TIMEOUT_SECS),
        download.next(),
    )
    .await
    .map_err(|_| format!("download chunk timed out after {DOWNLOAD_CHUNK_TIMEOUT_SECS}s"))?
    .map_err(|e| format!("download next failed: {e}"))?
    {
        file.write_all(&chunk)
            .map_err(|e| format!("cache file write failed: {e}"))?;
    }
    file.sync_all()
        .map_err(|e| format!("cache file sync failed: {e}"))?;

    Ok(cache_path)
}

fn encode_tgmtproto_object_id_v1(
    peer: &str,
    msg_id: i32,
    doc_id: i64,
    access_hash: i64,
) -> Result<String, String> {
    let payload = TgMtProtoObjectIdV1Payload {
        peer: peer.to_string(),
        msg_id: msg_id.to_string(),
        doc_id: doc_id.to_string(),
        access_hash: access_hash.to_string(),
    };
    let json = serde_json::to_vec(&payload).map_err(|e| format!("json failed: {e}"))?;
    let b64 = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(json);
    Ok(format!("{TG_MTPROTO_OBJECT_ID_PREFIX_V1}{b64}"))
}

#[derive(Debug)]
struct TgMtProtoObjectIdV1 {
    peer: String,
    msg_id: i32,
    doc_id: i64,
    access_hash: i64,
}

fn parse_tgmtproto_object_id_v1(encoded: &str) -> Result<TgMtProtoObjectIdV1, String> {
    let b64 = encoded
        .strip_prefix(TG_MTPROTO_OBJECT_ID_PREFIX_V1)
        .ok_or_else(|| format!("invalid object_id (missing {TG_MTPROTO_OBJECT_ID_PREFIX_V1})"))?;

    if b64.contains('+') || b64.contains('@') {
        return Err("invalid object_id (contains '+' or '@')".to_string());
    }

    let json = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(b64.as_bytes())
        .map_err(|e| format!("bad base64url: {e}"))?;

    let payload: TgMtProtoObjectIdV1Payload =
        serde_json::from_slice(&json).map_err(|e| format!("bad json: {e}"))?;

    let msg_id = payload
        .msg_id
        .parse::<i32>()
        .map_err(|_| "bad msgId".to_string())?;
    let doc_id = payload
        .doc_id
        .parse::<i64>()
        .map_err(|_| "bad docId".to_string())?;
    let access_hash = payload
        .access_hash
        .parse::<i64>()
        .map_err(|_| "bad accessHash".to_string())?;

    Ok(TgMtProtoObjectIdV1 {
        peer: payload.peer,
        msg_id,
        doc_id,
        access_hash,
    })
}

fn extract_document_id(media: &Media) -> Result<(i64, i64), String> {
    let doc_media = match media {
        Media::Document(d) => d,
        other => return Err(format!("expected document media, got {other:?}")),
    };

    let doc = doc_media
        .raw
        .document
        .clone()
        .ok_or_else(|| "missing document in media".to_string())?;

    match doc {
        tl::enums::Document::Document(d) => Ok((d.id, d.access_hash)),
        tl::enums::Document::Empty(_) => Err("unexpected empty document".to_string()),
    }
}
