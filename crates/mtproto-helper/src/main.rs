use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::Arc;

use base64::Engine;
use grammers_client::client::files::MAX_CHUNK_SIZE;
use grammers_client::grammers_tl_types as tl;
use grammers_client::session::defs::{PeerAuth, PeerId, PeerRef};
use grammers_client::session::storages::TlSession;
use grammers_client::types::{Media, Peer};
use grammers_client::{Client, InputMessage};
use grammers_mtsender::SenderPool;
use serde::{Deserialize, Serialize};
use tokio::time::{Duration, timeout};

const TG_MTPROTO_OBJECT_ID_PREFIX_V1: &str = "tgmtproto:v1:";
const INIT_IS_AUTHORIZED_TIMEOUT_SECS: u64 = 120;
const INIT_BOT_SIGN_IN_TIMEOUT_SECS: u64 = 120;
const INIT_RESOLVE_CHAT_TIMEOUT_SECS: u64 = 60;
const UPLOAD_SEND_MESSAGE_TIMEOUT_SECS: u64 = 60;
const DOWNLOAD_GET_MESSAGE_TIMEOUT_SECS: u64 = 60;
const DOWNLOAD_CHUNK_TIMEOUT_SECS: u64 = 60;

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

struct State {
    chat_id: String,
    cache_dir: PathBuf,
    session: Arc<TlSession>,
    client: Client,
    chat: Peer,
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

                let res = upload(s, req.filename, bytes).await;
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
    let SenderPool { runner, .. } = pool;
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

    let chat = timeout(
        Duration::from_secs(INIT_RESOLVE_CHAT_TIMEOUT_SECS),
        resolve_chat(&client, &req.chat_id),
    )
    .await
    .map_err(|_| {
        format!(
            "resolve_chat timed out after {INIT_RESOLVE_CHAT_TIMEOUT_SECS}s (check chat_id and bot dialog history)"
        )
    })?
    .map_err(|e| format!("resolve chat failed: {e}"))?;

    std::fs::create_dir_all(&req.cache_dir).map_err(|e| format!("cache dir create failed: {e}"))?;

    Ok(State {
        chat_id: req.chat_id,
        cache_dir: req.cache_dir,
        session,
        client,
        chat,
    })
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
    let pinned = timeout(
        Duration::from_secs(DOWNLOAD_GET_MESSAGE_TIMEOUT_SECS),
        state.client.get_pinned_message(&state.chat),
    )
    .await
    .map_err(|_| {
        format!("get_pinned_message timed out after {DOWNLOAD_GET_MESSAGE_TIMEOUT_SECS}s")
    })?
    .map_err(|e| format!("get_pinned_message failed: {e}"))?;

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
    timeout(
        Duration::from_secs(UPLOAD_SEND_MESSAGE_TIMEOUT_SECS),
        state.client.pin_message(&state.chat, msg_id),
    )
    .await
    .map_err(|_| format!("pin_message timed out after {UPLOAD_SEND_MESSAGE_TIMEOUT_SECS}s"))?
    .map_err(|e| format!("pin_message failed: {e}"))?;
    Ok(())
}

async fn upload(state: &mut State, filename: String, bytes: Vec<u8>) -> Result<String, String> {
    let size = bytes.len();
    let mut stream = std::io::Cursor::new(bytes);
    let timeout_secs = upload_stream_timeout_secs(size);
    let uploaded = timeout(
        Duration::from_secs(timeout_secs),
        state.client.upload_stream(&mut stream, size, filename),
    )
    .await
    .map_err(|_| format!("upload_stream timed out after {timeout_secs}s"))?
    .map_err(|e| format!("upload_stream failed: {e}"))?;

    let msg = timeout(
        Duration::from_secs(UPLOAD_SEND_MESSAGE_TIMEOUT_SECS),
        state
            .client
            .send_message(&state.chat, InputMessage::new().text("").file(uploaded)),
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
            .get_messages_by_id(&state.chat, &[parsed.msg_id]),
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
