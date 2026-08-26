use std::collections::VecDeque;
use std::future::Future;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc as tokio_mpsc;

use super::{Storage, StorageProgress};
use crate::{Error, Result};

const TG_MTPROTO_OBJECT_ID_PREFIX_V1: &str = "tgmtproto:v1:";
const MTPROTO_HELPER_READ_TIMEOUT_SECS: u64 = 600;
// Upload progress events should arrive quickly (the helper emits a 0-byte heartbeat
// before the first network request). If nothing arrives for this long, treat it as
// a stalled helper and fail fast so the caller can retry/respawn instead of freezing.
const MTPROTO_HELPER_UPLOAD_EVENT_TIMEOUT_SECS: u64 = 45;
const MTPROTO_HELPER_SHUTDOWN_TIMEOUT_SECS: u64 = 2;
const MTPROTO_HELPER_STDERR_TAIL_MAX_BYTES: usize = 8 * 1024;

fn spawn_helper_stderr_reader(
    stderr: ChildStderr,
) -> (Arc<Mutex<VecDeque<u8>>>, Arc<AtomicBool>, JoinHandle<()>) {
    let tail = Arc::new(Mutex::new(VecDeque::with_capacity(
        MTPROTO_HELPER_STDERR_TAIL_MAX_BYTES,
    )));
    let reader_tail = Arc::clone(&tail);
    let done = Arc::new(AtomicBool::new(false));
    let reader_done = Arc::clone(&done);
    let reader = std::thread::spawn(move || {
        let mut stderr = BufReader::new(stderr);
        let mut buffer = [0u8; 1024];
        loop {
            let read = match stderr.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => read,
            };
            if let Ok(mut saved) = reader_tail.lock() {
                for byte in &buffer[..read] {
                    if saved.len() == MTPROTO_HELPER_STDERR_TAIL_MAX_BYTES {
                        saved.pop_front();
                    }
                    saved.push_back(*byte);
                }
            }
            let _ = std::io::stderr().write_all(&buffer[..read]);
        }
        reader_done.store(true, Ordering::Release);
    });
    (tail, done, reader)
}

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
    pub min_delay_ms: Option<u64>,
    pub max_concurrent_uploads: Option<usize>,
    pub helper_path: Option<PathBuf>,
}

pub struct TelegramMtProtoStorage {
    provider: String,
    chat_id: String,
    manager: Arc<MtProtoHelperManager>,
}

struct MtProtoHelperManager {
    api_id: i32,
    api_hash: String,
    bot_token: String,
    chat_id: String,
    cache_dir: PathBuf,
    min_delay_ms: Option<u64>,
    max_concurrent_uploads: Option<usize>,
    helper_path: PathBuf,
    session: Mutex<Option<Vec<u8>>>,
    helper_pool: MtProtoHelperPool,
}

impl TelegramMtProtoStorage {
    pub async fn connect(config: TelegramMtProtoStorageConfig) -> Result<Self> {
        let helper_path = config.helper_path.unwrap_or_else(|| {
            default_helper_path().unwrap_or_else(|| PathBuf::from("televybackup-mtproto-helper"))
        });

        let session_b64 = config
            .session
            .map(|b| base64::engine::general_purpose::STANDARD.encode(b));

        let api_id = config.api_id;
        let api_hash = config.api_hash;
        let bot_token = config.bot_token;
        let cache_dir = config.cache_dir;
        let chat_id = config.chat_id;
        let min_delay_ms = config.min_delay_ms;
        let max_concurrent_uploads = config.max_concurrent_uploads;

        let pool_size = max_concurrent_uploads.unwrap_or(1).clamp(1, 8);
        let mut helpers = Vec::with_capacity(pool_size);
        let mut primary_session_bytes = None::<Vec<u8>>;
        for i in 0..pool_size {
            let is_primary = i == 0;
            let mut helper = MtProtoHelper::spawn(&helper_path)?;
            helper.init(InitRequest {
                api_id,
                api_hash: api_hash.clone(),
                bot_token: bot_token.clone(),
                chat_id: chat_id.clone(),
                // MTProto sessions are not safe to use concurrently across multiple processes.
                // Only the primary helper (the one whose session we persist) should reuse the
                // stored session; additional helpers start with a fresh session and authenticate
                // via bot token.
                session_b64: if is_primary {
                    session_b64.clone()
                } else {
                    None
                },
                cache_dir: cache_dir.clone(),
                min_delay_ms,
                // Core distributes this cap across helpers. One file-part worker per document
                // keeps combined Telegram RPC concurrency within the configured global limit.
                max_concurrent_uploads: Some(1),
            })?;
            if is_primary {
                primary_session_bytes = helper.session_bytes();
            }
            helpers.push(PooledHelper { helper, is_primary });
        }

        Ok(Self {
            provider: config.provider,
            chat_id: chat_id.clone(),
            manager: Arc::new(MtProtoHelperManager {
                api_id,
                api_hash,
                bot_token,
                chat_id,
                cache_dir,
                min_delay_ms,
                max_concurrent_uploads: Some(1),
                helper_path,
                session: Mutex::new(primary_session_bytes),
                helper_pool: MtProtoHelperPool::new(helpers),
            }),
        })
    }

    pub fn session_bytes(&self) -> Option<Vec<u8>> {
        self.manager.session_bytes()
    }

    fn with_helper<T>(&self, f: impl FnOnce(&mut MtProtoHelper) -> Result<T>) -> Result<T> {
        maybe_block_in_place(|| self.manager.with_helper_blocking(f))
    }

    async fn upload_async<'a>(
        &self,
        request: UploadRequest,
        mut progress: Option<Box<dyn FnMut(StorageProgress) + Send + 'a>>,
    ) -> Result<String> {
        let (progress_tx, mut progress_rx) = tokio_mpsc::unbounded_channel();
        let manager = Arc::clone(&self.manager);
        let forward_progress = progress.is_some();
        let mut task = tokio::task::spawn_blocking(move || {
            manager.with_helper_blocking(|helper| {
                if forward_progress {
                    let mut relay = |update: StorageProgress| {
                        let _ = progress_tx.send(update);
                    };
                    helper.upload_with_progress(request, Some(&mut relay))
                } else {
                    helper.upload(request)
                }
            })
        });
        let mut progress_open = forward_progress;

        loop {
            tokio::select! {
                biased;
                result = &mut task => {
                    let result = result.map_err(|error| Error::Telegram {
                        message: format!("mtproto helper upload task failed: {error}"),
                    })?;
                    while let Ok(update) = progress_rx.try_recv() {
                        if let Some(callback) = progress.as_mut() {
                            callback(update);
                        }
                    }
                    return result;
                }
                update = progress_rx.recv(), if progress_open => {
                    match update {
                        Some(update) => {
                            if let Some(callback) = progress.as_mut() {
                                callback(update);
                            }
                        }
                        None => progress_open = false,
                    }
                }
            }
        }
    }

    pub fn pinned_object_id(&self) -> Result<Option<String>> {
        self.with_helper(|helper| helper.get_pinned())
    }

    pub fn pin_message_id(&self, msg_id: i32) -> Result<()> {
        self.with_helper(|helper| helper.pin(msg_id))?;
        Ok(())
    }

    pub fn list_dialogs(
        &self,
        limit: usize,
        include_users: bool,
    ) -> Result<Vec<TelegramDialogInfo>> {
        self.with_helper(|helper| helper.list_dialogs(limit, include_users))
    }

    pub fn wait_for_chat(
        &self,
        timeout_secs: u64,
        include_users: bool,
    ) -> Result<TelegramDialogInfo> {
        self.with_helper(|helper| helper.wait_for_chat(timeout_secs, include_users))
    }
}

impl MtProtoHelperManager {
    fn session_bytes(&self) -> Option<Vec<u8>> {
        self.session.lock().ok().and_then(|guard| guard.clone())
    }

    fn should_respawn_helper_after(err: &Error) -> bool {
        match err {
            Error::Telegram { message } => {
                message.contains("mtproto helper")
                    || message.to_ascii_lowercase().contains("timed out")
                    || message.contains("save_file_part failed")
                    || message.contains("save_big_file_part failed")
            }
            _ => false,
        }
    }

    fn replace_helper_locked(&self, helper: &mut MtProtoHelper, is_primary: bool) -> Result<()> {
        helper.shutdown_best_effort();

        let session_b64 = if is_primary {
            self.session_bytes()
                .map(|b| base64::engine::general_purpose::STANDARD.encode(b))
        } else {
            None
        };

        let mut new_helper = MtProtoHelper::spawn(&self.helper_path)?;
        new_helper.init(InitRequest {
            api_id: self.api_id,
            api_hash: self.api_hash.clone(),
            bot_token: self.bot_token.clone(),
            chat_id: self.chat_id.clone(),
            session_b64,
            cache_dir: self.cache_dir.clone(),
            min_delay_ms: self.min_delay_ms,
            max_concurrent_uploads: self.max_concurrent_uploads,
        })?;

        *helper = new_helper;
        if is_primary {
            *self.session.lock().map_err(|_| Error::Telegram {
                message: "mtproto helper session lock poisoned".to_string(),
            })? = helper.session_bytes();
        }
        Ok(())
    }

    fn ensure_helper_running_locked(
        &self,
        helper: &mut MtProtoHelper,
        is_primary: bool,
    ) -> Result<()> {
        if helper.has_exited() {
            self.replace_helper_locked(helper, is_primary)?;
        }
        Ok(())
    }

    fn with_helper_blocking<T>(
        &self,
        f: impl FnOnce(&mut MtProtoHelper) -> Result<T>,
    ) -> Result<T> {
        // This runs on Tokio's blocking pool for document uploads. Synchronous control-plane
        // operations still enter through `TelegramMtProtoStorage::with_helper`.
        let mut pooled = self.helper_pool.checkout()?;
        let helper = &mut pooled.helper;

        // Always make sure we don't keep using a dead helper between runs.
        if let Err(e) = self.ensure_helper_running_locked(helper, pooled.is_primary) {
            self.helper_pool.checkin(pooled);
            return Err(e);
        }

        // Ensure the helper is returned to the pool even if the caller panics (should be rare,
        // but better than permanently reducing pool capacity).
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(helper)));

        // Only the primary helper is allowed to update the persisted session; secondary helpers
        // run with independent sessions to avoid MTProto seqno/message_id divergence across
        // processes.
        if pooled.is_primary {
            *self.session.lock().map_err(|_| Error::Telegram {
                message: "mtproto helper session lock poisoned".to_string(),
            })? = helper.session_bytes();
        }

        // If the helper process itself is unhealthy, respawn it so the next run can proceed
        // without needing a full app/daemon restart.
        if let Ok(Err(ref e)) = res
            && Self::should_respawn_helper_after(e)
        {
            let _ = self.replace_helper_locked(helper, pooled.is_primary);
        }

        self.helper_pool.checkin(pooled);

        match res {
            Ok(v) => v,
            Err(panic) => std::panic::resume_unwind(panic),
        }
    }
}

struct PooledHelper {
    helper: MtProtoHelper,
    is_primary: bool,
}

struct MtProtoHelperPool {
    inner: Mutex<Vec<PooledHelper>>,
    available: Condvar,
}

impl MtProtoHelperPool {
    fn new(helpers: Vec<PooledHelper>) -> Self {
        Self {
            inner: Mutex::new(helpers),
            available: Condvar::new(),
        }
    }

    fn checkout(&self) -> Result<PooledHelper> {
        let mut guard = self.inner.lock().map_err(|_| Error::Telegram {
            message: "mtproto helper pool lock poisoned".to_string(),
        })?;
        loop {
            if let Some(h) = guard.pop() {
                return Ok(h);
            }
            guard = self.available.wait(guard).map_err(|_| Error::Telegram {
                message: "mtproto helper pool lock poisoned".to_string(),
            })?;
        }
    }

    fn checkin(&self, helper: PooledHelper) {
        if let Ok(mut guard) = self.inner.lock() {
            guard.push(helper);
            self.available.notify_one();
        }
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
            self.upload_async(
                UploadRequest {
                    filename: filename.to_string(),
                    bytes,
                },
                None,
            )
            .await
        })
    }

    fn upload_document_with_progress<'a>(
        &'a self,
        filename: &'a str,
        bytes: Vec<u8>,
        progress: Option<Box<dyn FnMut(StorageProgress) + Send + 'a>>,
    ) -> Pin<Box<dyn Future<Output = Result<String>> + Send + 'a>> {
        Box::pin(async move {
            self.upload_async(
                UploadRequest {
                    filename: filename.to_string(),
                    bytes,
                },
                progress,
            )
            .await
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

            let resp = self.with_helper(|helper| {
                helper.download(DownloadRequest {
                    object_id: object_id.to_string(),
                })
            })?;
            Ok(resp)
        })
    }

    fn download_document_with_progress<'a>(
        &'a self,
        object_id: &'a str,
        mut progress: Option<Box<dyn FnMut(StorageProgress) + Send + 'a>>,
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

            let resp = self.with_helper(|helper| {
                let progress = progress
                    .as_deref_mut()
                    .map(|cb| cb as &mut dyn FnMut(StorageProgress));
                helper.download_with_progress(
                    DownloadRequest {
                        object_id: object_id.to_string(),
                    },
                    progress,
                )
            })?;
            Ok(resp)
        })
    }
}

impl Drop for MtProtoHelper {
    fn drop(&mut self) {
        self.shutdown_best_effort();
    }
}

fn maybe_block_in_place<T>(f: impl FnOnce() -> T) -> T {
    match tokio::runtime::Handle::try_current() {
        Ok(handle)
            if matches!(
                handle.runtime_flavor(),
                tokio::runtime::RuntimeFlavor::MultiThread
            ) =>
        {
            tokio::task::block_in_place(f)
        }
        _ => f(),
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
    #[cfg(unix)]
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(unix)]
    use tempfile::TempDir;

    #[cfg(unix)]
    const FAKE_HELPER_SESSION_B64: &str = "c2Vzc2lvbg==";
    #[cfg(unix)]
    const PRIMARY_SESSION_B64: &str = "c2F2ZWQ=";

    #[cfg(unix)]
    #[derive(Clone, Copy)]
    enum FakeHelperMode {
        Graceful,
        HangAfterShutdownAck,
        DelayedUploads,
        CrashOnUpload,
    }

    #[cfg(unix)]
    struct FakeHelperEnv {
        _tempdir: TempDir,
        script_path: PathBuf,
        requests_path: PathBuf,
        events_path: PathBuf,
    }

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

    #[cfg(unix)]
    fn write_fake_helper(mode: FakeHelperMode) -> FakeHelperEnv {
        let tempdir = tempfile::tempdir().unwrap();
        let script_path = tempdir.path().join("fake-helper.sh");
        let requests_path = tempdir.path().join("requests.log");
        let events_path = tempdir.path().join("events.log");
        fs::write(&requests_path, "").unwrap();
        fs::write(&events_path, "").unwrap();

        let mode = match mode {
            FakeHelperMode::Graceful => "graceful",
            FakeHelperMode::HangAfterShutdownAck => "hang_after_ack",
            FakeHelperMode::DelayedUploads => "delayed_uploads",
            FakeHelperMode::CrashOnUpload => "crash_on_upload",
        };
        let script = format!(
            r#"#!/bin/sh
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
REQUESTS="$SCRIPT_DIR/requests.log"
EVENTS="$SCRIPT_DIR/events.log"
MODE="{mode}"

touch "$REQUESTS" "$EVENTS"

while IFS= read -r line; do
  printf '%s\n' "$line" >> "$REQUESTS"
  case "$line" in
    *'"cmd":"init"'*)
      printf '%s\n' '{{"ok":true,"session":"{FAKE_HELPER_SESSION_B64}"}}'
      ;;
    *'"cmd":"shutdown"'*)
      printf 'shutdown\n' >> "$EVENTS"
      printf '%s\n' '{{"ok":true,"session":"{FAKE_HELPER_SESSION_B64}"}}'
      if [ "$MODE" = "hang_after_ack" ]; then
        while :; do
          sleep 3600
        done
      fi
      exit 0
      ;;
    *'"cmd":"upload"'*)
      size=$(printf '%s\n' "$line" | sed -n 's/.*"size":\([0-9][0-9]*\).*/\1/p')
      dd bs=1 count="$size" of=/dev/null 2>/dev/null
      printf 'upload_start\n' >> "$EVENTS"
      if [ "$MODE" = "crash_on_upload" ]; then
        printf 'intentional helper crash\n' >&2
        exit 42
      fi
      printf '%s\n' '{{"ok":true,"event":"upload_progress","bytesUploaded":1}}'
      if [ "$MODE" = "delayed_uploads" ]; then
        sleep 0.3
      fi
      printf '%s\n' '{{"ok":true,"objectId":"fake-object"}}'
      ;;
    *)
      printf 'unexpected:%s\n' "$line" >> "$EVENTS"
      printf '%s\n' '{{"ok":false,"error":"unexpected request"}}'
      ;;
  esac
done

printf 'eof\n' >> "$EVENTS"
"#,
        );
        fs::write(&script_path, script).unwrap();
        fs::set_permissions(&script_path, fs::Permissions::from_mode(0o755)).unwrap();

        FakeHelperEnv {
            _tempdir: tempdir,
            script_path,
            requests_path,
            events_path,
        }
    }

    #[cfg(unix)]
    fn fake_init_request(cache_dir: &Path, session_b64: Option<String>) -> InitRequest {
        InitRequest {
            api_id: 1,
            api_hash: "hash".to_string(),
            bot_token: "bot".to_string(),
            chat_id: String::new(),
            session_b64,
            cache_dir: cache_dir.to_path_buf(),
            min_delay_ms: None,
            max_concurrent_uploads: Some(1),
        }
    }

    #[cfg(unix)]
    async fn connect_fake_storage(
        script_path: &Path,
        cache_dir: &Path,
        session: Option<Vec<u8>>,
        max_concurrent_uploads: Option<usize>,
    ) -> TelegramMtProtoStorage {
        TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
            provider: "telegram_mtproto".to_string(),
            api_id: 1,
            api_hash: "hash".to_string(),
            bot_token: "bot".to_string(),
            chat_id: String::new(),
            session,
            cache_dir: cache_dir.to_path_buf(),
            min_delay_ms: None,
            max_concurrent_uploads,
            helper_path: Some(script_path.to_path_buf()),
        })
        .await
        .unwrap()
    }

    #[cfg(unix)]
    fn process_exists(pid: u32) -> bool {
        let output = Command::new("ps")
            .args(["-p", &pid.to_string(), "-o", "pid="])
            .output()
            .unwrap();
        output.status.success() && !String::from_utf8_lossy(&output.stdout).trim().is_empty()
    }

    #[cfg(unix)]
    fn wait_for_process_exit(pid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if !process_exists(pid) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        !process_exists(pid)
    }

    #[cfg(unix)]
    fn wait_for_request_count(path: &Path, expected: usize) -> String {
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let content = fs::read_to_string(path).unwrap();
            let count = content
                .lines()
                .filter(|line| line.contains(r#""cmd":"init""#))
                .count();
            if count >= expected {
                return content;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        fs::read_to_string(path).unwrap()
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn mtproto_storage_drop_gracefully_shutdowns_helper() {
        let fake = write_fake_helper(FakeHelperMode::Graceful);
        let cache_dir = fake
            .script_path
            .parent()
            .unwrap()
            .join("cache-storage-drop");
        let storage = connect_fake_storage(&fake.script_path, &cache_dir, None, Some(1)).await;
        assert_eq!(storage.session_bytes(), Some(b"session".to_vec()));

        let pid = {
            let guard = storage.manager.helper_pool.inner.lock().unwrap();
            guard[0].helper.child.id()
        };

        drop(storage);

        assert!(wait_for_process_exit(pid, Duration::from_secs(5)));
        let events = fs::read_to_string(&fake.events_path).unwrap();
        assert_eq!(events.lines().collect::<Vec<_>>(), vec!["shutdown"]);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn connect_reuses_session_only_for_primary_helper() {
        let fake = write_fake_helper(FakeHelperMode::Graceful);
        let cache_dir = fake.script_path.parent().unwrap().join("cache-connect");
        let storage = connect_fake_storage(
            &fake.script_path,
            &cache_dir,
            Some(b"saved".to_vec()),
            Some(2),
        )
        .await;

        assert_eq!(storage.session_bytes(), Some(b"session".to_vec()));

        let requests = wait_for_request_count(&fake.requests_path, 2);
        let init_lines: Vec<_> = requests
            .lines()
            .filter(|line| line.contains(r#""cmd":"init""#))
            .collect();
        assert_eq!(init_lines.len(), 2);
        assert_eq!(
            init_lines
                .iter()
                .filter(|line| line.contains(&format!(r#""session":"{PRIMARY_SESSION_B64}""#)))
                .count(),
            1
        );
        assert_eq!(
            init_lines
                .iter()
                .filter(|line| line.contains(r#""session":null"#))
                .count(),
            1
        );
        assert!(
            init_lines
                .iter()
                .all(|line| line.contains(r#""maxConcurrentUploads":1"#))
        );

        drop(storage);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn mtproto_storage_uploads_use_independent_blocking_tasks() {
        let fake = write_fake_helper(FakeHelperMode::DelayedUploads);
        let cache_dir = fake
            .script_path
            .parent()
            .unwrap()
            .join("cache-upload-concurrency");
        let storage = connect_fake_storage(&fake.script_path, &cache_dir, None, Some(2)).await;
        let progress_a = std::sync::atomic::AtomicUsize::new(0);
        let progress_b = std::sync::atomic::AtomicUsize::new(0);
        let started = Instant::now();

        let (first, second) = tokio::join!(
            storage.upload_document_with_progress(
                "first.bin",
                vec![1],
                Some(Box::new(|_| {
                    progress_a.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                })),
            ),
            storage.upload_document_with_progress(
                "second.bin",
                vec![2],
                Some(Box::new(|_| {
                    progress_b.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                })),
            ),
        );

        assert_eq!(first.unwrap(), "fake-object");
        assert_eq!(second.unwrap(), "fake-object");
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "uploads should overlap instead of serializing on the first future poll"
        );
        assert_eq!(progress_a.load(std::sync::atomic::Ordering::Relaxed), 1);
        assert_eq!(progress_b.load(std::sync::atomic::Ordering::Relaxed), 1);
        let events = fs::read_to_string(&fake.events_path).unwrap();
        assert_eq!(
            events
                .lines()
                .filter(|line| *line == "upload_start")
                .count(),
            2
        );
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn mtproto_storage_forwards_progress_queued_with_completion() {
        let fake = write_fake_helper(FakeHelperMode::Graceful);
        let cache_dir = fake
            .script_path
            .parent()
            .unwrap()
            .join("cache-upload-progress");
        let storage = connect_fake_storage(&fake.script_path, &cache_dir, None, Some(1)).await;
        let progress = std::sync::atomic::AtomicUsize::new(0);

        let object_id = storage
            .upload_document_with_progress(
                "progress.bin",
                vec![1],
                Some(Box::new(|_| {
                    progress.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                })),
            )
            .await
            .unwrap();

        assert_eq!(object_id, "fake-object");
        assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), 1);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn mtproto_storage_reports_helper_exit_status_and_stderr() {
        let fake = write_fake_helper(FakeHelperMode::CrashOnUpload);
        let cache_dir = fake
            .script_path
            .parent()
            .unwrap()
            .join("cache-upload-crash");
        let storage = connect_fake_storage(&fake.script_path, &cache_dir, None, Some(1)).await;

        let error = storage
            .upload_document("crash.bin", vec![1])
            .await
            .expect_err("crashing helper should fail the upload");
        let message = error.to_string();

        assert!(message.contains("mtproto helper closed stdout"));
        assert!(message.contains("exit status: 42"));
        assert!(message.contains("intentional helper crash"));
    }

    #[cfg(unix)]
    #[test]
    fn mtproto_helper_drop_kills_when_shutdown_hangs() {
        let fake = write_fake_helper(FakeHelperMode::HangAfterShutdownAck);
        let cache_dir = fake.script_path.parent().unwrap().join("cache-helper-drop");
        let mut helper = MtProtoHelper::spawn(&fake.script_path).unwrap();
        helper
            .init(fake_init_request(
                &cache_dir,
                Some(PRIMARY_SESSION_B64.to_string()),
            ))
            .unwrap();
        let pid = helper.child.id();

        drop(helper);

        assert!(wait_for_process_exit(pid, Duration::from_secs(5)));
        let events = fs::read_to_string(&fake.events_path).unwrap();
        assert!(events.lines().any(|line| line == "shutdown"));
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
enum Request {
    Init(InitRequest),
    Shutdown,
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
    #[serde(skip_serializing_if = "Option::is_none", rename = "minDelayMs")]
    min_delay_ms: Option<u64>,
    #[serde(
        skip_serializing_if = "Option::is_none",
        rename = "maxConcurrentUploads"
    )]
    max_concurrent_uploads: Option<usize>,
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
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr_tail: Arc<Mutex<VecDeque<u8>>>,
    stderr_reader_done: Arc<AtomicBool>,
    stderr_reader: Option<JoinHandle<()>>,
    session_b64: Option<String>,
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
            .stderr(Stdio::piped())
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
        let stderr = child.stderr.take().ok_or_else(|| Error::InvalidConfig {
            message: "mtproto helper missing stderr".to_string(),
        })?;
        let (stderr_tail, stderr_reader_done, stderr_reader) = spawn_helper_stderr_reader(stderr);

        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            stderr_tail,
            stderr_reader_done,
            stderr_reader: Some(stderr_reader),
            session_b64: None,
        })
    }

    fn has_exited(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(Some(_)) => true,
            Ok(None) => false,
            Err(_) => true,
        }
    }

    fn kill_best_effort(&mut self) {
        let _ = self.child.kill();
        // Avoid blocking indefinitely; the caller may respawn immediately after this.
        for _ in 0..50 {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                Err(_) => break,
            }
        }
        self.join_stderr_reader();
    }

    fn wait_for_exit_best_effort(&mut self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return true,
                Ok(None) => {
                    if Instant::now() >= deadline {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return true,
            }
        }
    }

    fn shutdown_best_effort(&mut self) {
        if self.has_exited() {
            self.join_stderr_reader();
            return;
        }

        if self.send_json(&Request::Shutdown).is_ok()
            && let Ok(env) = self.read_json_line_with_timeout(MTPROTO_HELPER_SHUTDOWN_TIMEOUT_SECS)
        {
            let _ = self.apply_session(&env);
        }

        if !self
            .wait_for_exit_best_effort(Duration::from_secs(MTPROTO_HELPER_SHUTDOWN_TIMEOUT_SECS))
        {
            self.kill_best_effort();
        }
        self.join_stderr_reader();
    }

    fn join_stderr_reader(&mut self) {
        let deadline = Instant::now() + Duration::from_millis(100);
        while !self.stderr_reader_done.load(Ordering::Acquire) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        if self.stderr_reader_done.load(Ordering::Acquire)
            && let Some(reader) = self.stderr_reader.take()
        {
            let _ = reader.join();
        } else {
            // A descendant may inherit the stderr pipe after the helper exits. Do not let
            // cleanup wait forever for that unrelated process; dropping the handle detaches
            // the bounded diagnostic reader.
            let _ = self.stderr_reader.take();
        }
    }

    fn stderr_tail_for_diagnostic(&self) -> Option<String> {
        let bytes: Vec<u8> = self.stderr_tail.lock().ok()?.iter().copied().collect();
        let mut text = String::from_utf8_lossy(&bytes).trim().to_string();
        if text.is_empty() {
            return None;
        }
        text.retain(|c| c == '\n' || c == '\r' || c == '\t' || !c.is_control());
        (!text.is_empty()).then_some(text)
    }

    fn read_error(&mut self, error: std::io::Error) -> Error {
        let exit_detail = match self.child.try_wait() {
            Ok(Some(status)) => {
                self.join_stderr_reader();
                format!("helper exit status: {status}")
            }
            Ok(None) => "helper process is still running".to_string(),
            Err(wait_error) => format!("helper exit status unavailable: {wait_error}"),
        };
        let stderr_detail = self
            .stderr_tail_for_diagnostic()
            .map(|tail| format!("; helper stderr tail: {tail}"))
            .unwrap_or_default();
        Error::Telegram {
            message: format!("mtproto helper read failed: {error}; {exit_detail}{stderr_detail}"),
        }
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

    fn upload(&mut self, req: UploadRequest) -> Result<String> {
        self.upload_with_progress(req, None)
    }

    fn upload_with_progress(
        &mut self,
        req: UploadRequest,
        mut on_progress: Option<&mut dyn FnMut(StorageProgress)>,
    ) -> Result<String> {
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
            let env = self.read_json_line_with_timeout(MTPROTO_HELPER_UPLOAD_EVENT_TIMEOUT_SECS)?;
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
                    let net_bytes = env.data.get("netBytesOut").and_then(|v| v.as_u64());
                    (**cb)(StorageProgress { bytes, net_bytes });
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

            return Ok(object_id);
        }
    }

    fn download(&mut self, req: DownloadRequest) -> Result<Vec<u8>> {
        self.download_with_progress(req, None)
    }

    fn download_with_progress(
        &mut self,
        req: DownloadRequest,
        mut on_progress: Option<&mut dyn FnMut(StorageProgress)>,
    ) -> Result<Vec<u8>> {
        self.send_json(&Request::Download(req))?;
        let mut saw_progress_event = false;
        // The helper may resume from an on-disk cache and emit `bytesDownloaded` starting at the
        // cached length (non-zero). For UI bandwidth indicators we want "bytes downloaded in this
        // invocation" so we normalize the progress stream to start at 0.
        let mut progress_base: Option<u64> = None;
        let env = loop {
            let env = self.read_json_line()?;
            self.apply_session(&env)?;
            if !env.ok {
                return Err(Error::Telegram {
                    message: env
                        .error
                        .unwrap_or_else(|| "mtproto download failed".to_string()),
                });
            }

            let event = env
                .data
                .get("event")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            if event == "download_progress" {
                saw_progress_event = true;
                if let (Some(bytes), Some(cb)) = (
                    env.data.get("bytesDownloaded").and_then(|v| v.as_u64()),
                    on_progress.as_mut(),
                ) {
                    let base = *progress_base.get_or_insert(bytes);
                    let net_bytes = env.data.get("netBytesIn").and_then(|v| v.as_u64());
                    (**cb)(StorageProgress {
                        bytes: bytes.saturating_sub(base),
                        net_bytes,
                    });
                }
                continue;
            }

            break env;
        };

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

        let size_usize = size as usize;
        let mut bytes = vec![0u8; size_usize];

        // For older helpers, we only learn about download progress by reading the payload bytes.
        // Newer helpers emit `download_progress` events while they download into a local cache, so
        // reporting progress here would create unrealistic spikes and even "rewind" the counter.
        const READ_CHUNK: usize = 256 * 1024;
        let mut read = 0usize;
        while read < size_usize {
            let end = (read + READ_CHUNK).min(size_usize);
            self.stdout
                .read_exact(&mut bytes[read..end])
                .map_err(|e| Error::Telegram {
                    message: format!("mtproto download read failed: {e}"),
                })?;
            read = end;
            if !saw_progress_event && let Some(cb) = on_progress.as_mut() {
                (**cb)(StorageProgress {
                    bytes: read as u64,
                    net_bytes: None,
                });
            }
        }

        Ok(bytes)
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
        self.read_json_line_with_timeout(MTPROTO_HELPER_READ_TIMEOUT_SECS)
    }

    fn read_json_line_with_timeout(&mut self, timeout_secs: u64) -> Result<ResponseEnvelope> {
        enum ReadOutcome {
            Line(String),
            ReadError(std::io::Error),
            TimedOut,
            Disconnected,
        }

        let outcome = {
            let (child, stdout) = (&mut self.child, &mut self.stdout);
            let (tx, rx) = mpsc::channel::<std::io::Result<String>>();
            std::thread::scope(|s| {
                s.spawn(|| {
                    let mut line = String::new();
                    let res = stdout.read_line(&mut line).and_then(|n| {
                        if n == 0 {
                            Err(std::io::Error::new(
                                std::io::ErrorKind::UnexpectedEof,
                                "mtproto helper closed stdout",
                            ))
                        } else {
                            Ok(line)
                        }
                    });
                    let _ = tx.send(res);
                });

                match rx.recv_timeout(Duration::from_secs(timeout_secs)) {
                    Ok(Ok(line)) => ReadOutcome::Line(line),
                    Ok(Err(error)) => ReadOutcome::ReadError(error),
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        // The helper became unresponsive. Kill it so the blocked read unblocks,
                        // then let the caller decide whether to retry after respawn.
                        let _ = child.kill();
                        for _ in 0..50 {
                            match child.try_wait() {
                                Ok(Some(_)) => break,
                                Ok(None) => std::thread::sleep(Duration::from_millis(100)),
                                Err(_) => break,
                            }
                        }
                        ReadOutcome::TimedOut
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => ReadOutcome::Disconnected,
                }
            })
        };

        match outcome {
            ReadOutcome::Line(line) => serde_json::from_str::<ResponseEnvelope>(line.trim_end())
                .map_err(|error| Error::Telegram {
                    message: format!("mtproto helper invalid response: {error}"),
                }),
            ReadOutcome::ReadError(error) => Err(self.read_error(error)),
            ReadOutcome::TimedOut => {
                self.join_stderr_reader();
                Err(Error::Telegram {
                    message: format!(
                        "mtproto helper timed out waiting for response after {timeout_secs}s"
                    ),
                })
            }
            ReadOutcome::Disconnected => Err(Error::Telegram {
                message: "mtproto helper response channel disconnected".to_string(),
            }),
        }
    }
}
