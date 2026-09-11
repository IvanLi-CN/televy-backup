use std::collections::HashMap;
use std::convert::Infallible;
use std::fmt::Debug;
use std::io::SeekFrom;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use base64::Engine;
use bytes::{Buf, Bytes};
use chrono::{DateTime, Utc};
use dav_server::DavHandler;
use dav_server::DavMethod;
use dav_server::DavMethodSet;
use dav_server::davpath::DavPath;
use dav_server::fs::{
    DavDirEntry, DavFile, DavFileSystem, DavMetaData, FsError, FsFuture, FsResult, FsStream,
    OpenOptions, ReadDirMeta,
};
use futures_util::stream;
use getrandom::getrandom;
use hyper::Response;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde::Deserialize;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use televy_backup_core::config::{SettingsV2, Target, endpoint_provider};
use televy_backup_core::control::{ControlError, ControlRequest};
use televy_backup_core::snapshot_browsing::{
    BrowseSnapshot, DIAGNOSTICS_DIRECTORY, SnapshotBrowseCache, SnapshotContentReader,
    UNAVAILABLE_ENTRIES_FILE,
};
use televy_backup_core::{TelegramMtProtoStorage, TelegramMtProtoStorageConfig};

#[allow(dead_code)]
const MAX_HTTP_REQUEST: usize = 1024 * 1024;

#[derive(Debug, Deserialize)]
pub(crate) struct SnapshotBrowseMountParams {
    #[serde(alias = "targetId")]
    pub target_id: String,
    #[serde(default, alias = "allowCachedCatalog")]
    pub allow_cached_catalog: bool,
}

#[derive(Debug, Deserialize)]
pub(crate) struct SnapshotBrowseSessionParams {
    #[serde(alias = "sessionId")]
    pub session_id: String,
}

#[derive(Clone)]
struct BrowseSession {
    id: String,
    target_id: String,
    source_path: String,
    volume_name: String,
    capability: String,
    catalog_source: String,
    reader: Arc<SnapshotContentReader>,
    snapshots: Arc<Mutex<Vec<BrowseSnapshot>>>,
    metadata_overlay: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    diagnostics_json: Arc<Vec<u8>>,
    shutdown: CancellationToken,
}

#[derive(Clone)]
struct BrowseDavFs {
    session: Arc<BrowseSession>,
}

#[derive(Clone, Debug)]
enum BrowseFileSource {
    Snapshot { snapshot_id: String, path: String },
    Bytes(Vec<u8>),
}

#[derive(Clone, Debug)]
struct BrowseNode {
    meta: BrowseMeta,
    source: Option<BrowseFileSource>,
}

#[derive(Clone, Debug)]
struct BrowseMeta {
    size: u64,
    mtime_ms: i64,
    dir: bool,
}

#[derive(Debug)]
struct BrowseDirEntryImpl {
    name: Vec<u8>,
    meta: BrowseMeta,
}

struct BrowseDavFile {
    session: Arc<BrowseSession>,
    relative: String,
    source: BrowseFileSource,
    meta: BrowseMeta,
    position: u64,
    writable: bool,
    write_buffer: Vec<u8>,
}

impl Debug for BrowseDavFile {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowseDavFile")
            .field("relative", &self.relative)
            .field("position", &self.position)
            .field("writable", &self.writable)
            .finish()
    }
}

impl BrowseDavFs {
    fn path(path: &DavPath) -> Result<String, FsError> {
        String::from_utf8(path.as_bytes().to_vec())
            .map(|path| path.trim_matches('/').to_string())
            .map_err(|_| FsError::Forbidden)
    }

    async fn node(session: &BrowseSession, relative: &str) -> Result<BrowseNode, FsError> {
        if relative.is_empty() {
            return Ok(BrowseNode {
                meta: BrowseMeta {
                    size: 0,
                    mtime_ms: 0,
                    dir: true,
                },
                source: None,
            });
        }
        if relative == DIAGNOSTICS_DIRECTORY {
            return Ok(BrowseNode {
                meta: BrowseMeta {
                    size: 0,
                    mtime_ms: 0,
                    dir: true,
                },
                source: None,
            });
        }
        if relative == format!("{DIAGNOSTICS_DIRECTORY}/{UNAVAILABLE_ENTRIES_FILE}") {
            return Ok(BrowseNode {
                meta: BrowseMeta {
                    size: session.diagnostics_json.len() as u64,
                    mtime_ms: 0,
                    dir: false,
                },
                source: Some(BrowseFileSource::Bytes(
                    session.diagnostics_json.as_ref().clone(),
                )),
            });
        }
        if let Some(bytes) = session.metadata_overlay.lock().await.get(relative).cloned() {
            return Ok(BrowseNode {
                meta: BrowseMeta {
                    size: bytes.len() as u64,
                    mtime_ms: 0,
                    dir: false,
                },
                source: Some(BrowseFileSource::Bytes(bytes)),
            });
        }
        let Some((snapshot, snapshot_path)) = snapshot_path(relative, session).await else {
            return Err(FsError::NotFound);
        };
        let entry = session
            .reader
            .entry(&snapshot.snapshot_id, &snapshot_path)
            .await
            .map_err(|_| FsError::GeneralFailure)?
            .ok_or(FsError::NotFound)?;
        let dir = entry.kind == "dir";
        Ok(BrowseNode {
            meta: BrowseMeta {
                size: entry.size,
                mtime_ms: entry.mtime_ms,
                dir,
            },
            source: (!dir).then_some(BrowseFileSource::Snapshot {
                snapshot_id: snapshot.snapshot_id,
                path: snapshot_path,
            }),
        })
    }

    async fn children(
        session: &BrowseSession,
        relative: &str,
    ) -> Result<Vec<BrowseDirEntryImpl>, FsError> {
        let node = Self::node(session, relative).await?;
        if !node.meta.dir {
            return Err(FsError::Forbidden);
        }
        let mut entries = Vec::new();
        if relative.is_empty() {
            for snapshot in refresh_snapshots(session).await {
                entries.push(BrowseDirEntryImpl {
                    name: snapshot.display_name.into_bytes(),
                    meta: BrowseMeta {
                        size: 0,
                        mtime_ms: 0,
                        dir: true,
                    },
                });
            }
            entries.push(BrowseDirEntryImpl {
                name: DIAGNOSTICS_DIRECTORY.as_bytes().to_vec(),
                meta: BrowseMeta {
                    size: 0,
                    mtime_ms: 0,
                    dir: true,
                },
            });
        } else if relative == DIAGNOSTICS_DIRECTORY {
            entries.push(BrowseDirEntryImpl {
                name: UNAVAILABLE_ENTRIES_FILE.as_bytes().to_vec(),
                meta: BrowseMeta {
                    size: session.diagnostics_json.len() as u64,
                    mtime_ms: 0,
                    dir: false,
                },
            });
        } else if let Some((snapshot, snapshot_path)) = snapshot_path(relative, session).await {
            let children = session
                .reader
                .list_children(&snapshot.snapshot_id, &snapshot_path)
                .await
                .map_err(|_| FsError::GeneralFailure)?;
            entries.extend(children.into_iter().map(|entry| BrowseDirEntryImpl {
                name: entry.name.into_bytes(),
                meta: BrowseMeta {
                    size: entry.size,
                    mtime_ms: entry.mtime_ms,
                    dir: entry.kind == "dir",
                },
            }));
        }
        let overlays = session.metadata_overlay.lock().await;
        for path in overlays.keys() {
            let (parent, name) = path.rsplit_once('/').unwrap_or(("", path.as_str()));
            if parent == relative && !entries.iter().any(|entry| entry.name == name.as_bytes()) {
                let bytes = overlays.get(path).map_or(0, Vec::len) as u64;
                entries.push(BrowseDirEntryImpl {
                    name: name.as_bytes().to_vec(),
                    meta: BrowseMeta {
                        size: bytes,
                        mtime_ms: 0,
                        dir: false,
                    },
                });
            }
        }
        Ok(entries)
    }
}

impl DavFileSystem for BrowseDavFs {
    fn open<'a>(
        &'a self,
        path: &'a DavPath,
        options: OpenOptions,
    ) -> FsFuture<'a, Box<dyn DavFile>> {
        let relative = match Self::path(path) {
            Ok(relative) => relative,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let session = self.session.clone();
        Box::pin(async move {
            let node = match Self::node(&session, &relative).await {
                Ok(node) => node,
                Err(FsError::NotFound)
                    if options.write
                        && metadata_overlay_path(&relative)
                        && !real_snapshot_entry(&relative, &session).await =>
                {
                    BrowseNode {
                        meta: BrowseMeta {
                            size: 0,
                            mtime_ms: 0,
                            dir: false,
                        },
                        source: Some(BrowseFileSource::Bytes(Vec::new())),
                    }
                }
                Err(error) => return Err(error),
            };
            if node.meta.dir {
                return Err(FsError::Forbidden);
            }
            if options.write
                && (!metadata_overlay_path(&relative)
                    || real_snapshot_entry(&relative, &session).await)
            {
                return Err(FsError::Forbidden);
            }
            let source = node.source.ok_or(FsError::NotFound)?;
            let position = if options.append { node.meta.size } else { 0 };
            let write_buffer = match &source {
                BrowseFileSource::Bytes(bytes) if options.write => bytes.clone(),
                _ => Vec::new(),
            };
            Ok(Box::new(BrowseDavFile {
                session,
                relative,
                source,
                meta: node.meta,
                position,
                writable: options.write,
                write_buffer,
            }) as Box<dyn DavFile>)
        })
    }

    fn read_dir<'a>(
        &'a self,
        path: &'a DavPath,
        _meta: ReadDirMeta,
    ) -> FsFuture<'a, FsStream<Box<dyn DavDirEntry>>> {
        let relative = match Self::path(path) {
            Ok(relative) => relative,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let session = self.session.clone();
        Box::pin(async move {
            let entries = Self::children(&session, &relative).await?;
            let entries = entries
                .into_iter()
                .map(|entry| Ok(Box::new(entry) as Box<dyn DavDirEntry>));
            Ok(Box::pin(stream::iter(entries)) as FsStream<Box<dyn DavDirEntry>>)
        })
    }

    fn metadata<'a>(&'a self, path: &'a DavPath) -> FsFuture<'a, Box<dyn DavMetaData>> {
        let relative = match Self::path(path) {
            Ok(relative) => relative,
            Err(error) => return Box::pin(async move { Err(error) }),
        };
        let session = self.session.clone();
        Box::pin(async move {
            let node = Self::node(&session, &relative).await?;
            Ok(Box::new(node.meta) as Box<dyn DavMetaData>)
        })
    }
}

impl DavDirEntry for BrowseDirEntryImpl {
    fn name(&self) -> Vec<u8> {
        self.name.clone()
    }

    fn metadata(&'_ self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        let meta = self.meta.clone();
        Box::pin(async move { Ok(Box::new(meta) as Box<dyn DavMetaData>) })
    }
}

impl DavMetaData for BrowseMeta {
    fn len(&self) -> u64 {
        self.size
    }

    fn modified(&self) -> FsResult<SystemTime> {
        Ok(SystemTime::UNIX_EPOCH + Duration::from_millis(self.mtime_ms.max(0) as u64))
    }

    fn is_dir(&self) -> bool {
        self.dir
    }
}

impl DavFile for BrowseDavFile {
    fn metadata(&'_ mut self) -> FsFuture<'_, Box<dyn DavMetaData>> {
        let meta = self.meta.clone();
        Box::pin(async move { Ok(Box::new(meta) as Box<dyn DavMetaData>) })
    }

    fn write_buf(&'_ mut self, mut buf: Box<dyn Buf + Send>) -> FsFuture<'_, ()> {
        if buf.remaining() > 1024 * 1024 {
            return Box::pin(async { Err(FsError::TooLarge) });
        }
        let mut bytes = Vec::with_capacity(buf.remaining());
        while buf.has_remaining() {
            let chunk = buf.chunk();
            bytes.extend_from_slice(chunk);
            let len = chunk.len();
            buf.advance(len);
        }
        self.write_bytes(Bytes::from(bytes))
    }

    fn write_bytes(&'_ mut self, buf: Bytes) -> FsFuture<'_, ()> {
        if !self.writable {
            return Box::pin(async { Err(FsError::Forbidden) });
        }
        let position = self.position as usize;
        let bytes = buf.to_vec();
        if position.saturating_add(bytes.len()) > 1024 * 1024 {
            return Box::pin(async { Err(FsError::TooLarge) });
        }
        let write_buffer = &mut self.write_buffer;
        let current_position = &mut self.position;
        Box::pin(async move {
            if position > write_buffer.len() {
                write_buffer.resize(position, 0);
            }
            let end = position.saturating_add(bytes.len());
            if end > write_buffer.len() {
                write_buffer.resize(end, 0);
            }
            write_buffer[position..end].copy_from_slice(&bytes);
            *current_position = end as u64;
            Ok(())
        })
    }

    fn read_bytes(&'_ mut self, count: usize) -> FsFuture<'_, Bytes> {
        let session = self.session.clone();
        let source = self.source.clone();
        let position = &mut self.position;
        let max = self.meta.size;
        Box::pin(async move {
            if *position >= max || count == 0 {
                return Ok(Bytes::new());
            }
            let len = (count as u64).min(max - *position);
            let bytes = match source {
                BrowseFileSource::Bytes(bytes) => {
                    bytes[*position as usize..(*position + len) as usize].to_vec()
                }
                BrowseFileSource::Snapshot { snapshot_id, path } => {
                    session
                        .reader
                        .read_range(&snapshot_id, &path, *position, Some(len))
                        .await
                        .map_err(|_| FsError::GeneralFailure)?
                        .1
                }
            };
            *position += bytes.len() as u64;
            Ok(Bytes::from(bytes))
        })
    }

    fn seek(&'_ mut self, pos: SeekFrom) -> FsFuture<'_, u64> {
        let position = &mut self.position;
        let end = self.meta.size.max(self.write_buffer.len() as u64);
        Box::pin(async move {
            let next = match pos {
                SeekFrom::Start(value) => value,
                SeekFrom::Current(value) => {
                    if value < 0 {
                        position.saturating_sub(value.unsigned_abs())
                    } else {
                        position.saturating_add(value as u64)
                    }
                }
                SeekFrom::End(value) => {
                    if value < 0 {
                        end.saturating_sub(value.unsigned_abs())
                    } else {
                        end.saturating_add(value as u64)
                    }
                }
            };
            *position = next;
            Ok(next)
        })
    }

    fn flush(&'_ mut self) -> FsFuture<'_, ()> {
        if !self.writable {
            return Box::pin(async { Ok(()) });
        }
        let session = self.session.clone();
        let relative = self.relative.clone();
        let bytes = self.write_buffer.clone();
        Box::pin(async move {
            if bytes.len() > 1024 * 1024 {
                return Err(FsError::TooLarge);
            }
            session
                .metadata_overlay
                .lock()
                .await
                .insert(relative, bytes);
            Ok(())
        })
    }
}

#[derive(Clone)]
pub(crate) struct SnapshotBrowseService {
    config_root: PathBuf,
    data_root: PathBuf,
    settings: Arc<RwLock<SettingsV2>>,
    sessions: Arc<Mutex<HashMap<String, Arc<BrowseSession>>>>,
    target_sessions: Arc<Mutex<HashMap<String, String>>>,
    mount_lock: Arc<Mutex<()>>,
}

impl SnapshotBrowseService {
    pub(crate) fn new(
        config_root: PathBuf,
        data_root: PathBuf,
        settings: Arc<RwLock<SettingsV2>>,
    ) -> Self {
        Self {
            config_root,
            data_root,
            settings,
            sessions: Arc::new(Mutex::new(HashMap::new())),
            target_sessions: Arc::new(Mutex::new(HashMap::new())),
            mount_lock: Arc::new(Mutex::new(())),
        }
    }

    pub(crate) async fn handle(
        &self,
        request: &ControlRequest,
    ) -> Result<serde_json::Value, ControlError> {
        match request.method.as_str() {
            "snapshot.browse.mount" => {
                let params: SnapshotBrowseMountParams = decode_params(&request.params)?;
                self.mount(params).await
            }
            "snapshot.browse.status" => {
                let params: SnapshotBrowseSessionParams = decode_params(&request.params)?;
                self.status(&params.session_id).await
            }
            "snapshot.browse.unmount" => {
                let params: SnapshotBrowseSessionParams = decode_params(&request.params)?;
                self.unmount(&params.session_id).await
            }
            "snapshot.browse.recover" => {
                self.recover().await;
                Ok(serde_json::json!({ "recovered": true }))
            }
            _ => Err(ControlError::method_not_found(
                "snapshot browse method not found",
                serde_json::json!({ "method": request.method }),
            )),
        }
    }

    async fn mount(
        &self,
        params: SnapshotBrowseMountParams,
    ) -> Result<serde_json::Value, ControlError> {
        let _mount_guard = self.mount_lock.lock().await;
        let settings = self.settings.read().await.clone();
        let target = settings
            .targets
            .iter()
            .find(|target| target.id == params.target_id)
            .cloned()
            .ok_or_else(|| {
                ControlError::invalid_request(
                    "target was not found",
                    serde_json::json!({ "targetId": params.target_id }),
                )
            })?;
        if let Some(existing) = self.target_sessions.lock().await.get(&target.id).cloned() {
            return Err(ControlError {
                code: "snapshot.browse.already_mounted".to_string(),
                message: "This target is already mounted in Finder.".to_string(),
                retryable: false,
                details: serde_json::json!({ "sessionId": existing }),
            });
        }

        let (endpoint_db_path, filemap_dir) = find_target_index(&self.data_root, &target).await?;
        if params.allow_cached_catalog && !endpoint_db_path.is_file() {
            return Err(ControlError {
                code: "snapshot.browse.catalog_unavailable".to_string(),
                message: "The target catalog is not available locally for cached browsing."
                    .to_string(),
                retryable: true,
                details: serde_json::json!({}),
            });
        }
        let (storage, master_key) =
            connect_storage(&self.config_root, &self.data_root, &settings, &target)
                .await
                .map_err(sanitize_mount_error)?;
        if !params.allow_cached_catalog {
            let endpoint = settings
                .telegram_endpoints
                .iter()
                .find(|endpoint| endpoint.id == target.endpoint_id)
                .ok_or_else(|| {
                    ControlError::invalid_request(
                        "target endpoint was not found",
                        serde_json::json!({}),
                    )
                })?;
            if crate::is_likely_private_chat_id(&endpoint.chat_id) {
                return Err(ControlError {
                    code: "snapshot.browse.catalog_refresh_unavailable".to_string(),
                    message: "The remote backup catalog cannot be refreshed for a private Telegram chat. You can browse the last cached catalog.".to_string(),
                    retryable: true,
                    details: serde_json::json!({}),
                });
            }
            let dedupe_db_path = self
                .data_root
                .join("index")
                .join("dedupe")
                .join(format!("dedupe.{}.sqlite", target.endpoint_id));
            crate::preflight_remote_first_index_sync_daemon(
                &storage,
                &master_key,
                &target.id,
                &target.source_path,
                &endpoint_db_path,
                &filemap_dir,
                &dedupe_db_path,
                crate::is_likely_private_chat_id(&endpoint.chat_id),
                None,
                &CancellationToken::new(),
            )
            .await
            .map_err(|error| ControlError {
                code: "snapshot.browse.catalog_refresh_unavailable".to_string(),
                message: "The remote backup catalog could not be refreshed. You can browse the last cached catalog.".to_string(),
                retryable: true,
                details: serde_json::json!({ "sourceCode": error.code() }),
            })?;
        }
        let cache_root = self
            .data_root
            .join("cache")
            .join("snapshot-browsing")
            .join(&target.endpoint_id);
        let cache = Arc::new(SnapshotBrowseCache::new(
            cache_root,
            settings.snapshot_browsing.cache_max_bytes,
        ));
        let reader = Arc::new(SnapshotContentReader::new(
            endpoint_db_path,
            filemap_dir,
            Arc::new(storage),
            master_key,
            cache,
        ));
        let snapshots = reader
            .list_snapshots(&target.source_path)
            .await
            .map_err(core_error)?;
        if snapshots.is_empty() {
            return Err(ControlError {
                code: "snapshot.browse.catalog_empty".to_string(),
                message: "No retained snapshots are available for this target.".to_string(),
                retryable: false,
                details: serde_json::json!({}),
            });
        }
        let mut catalog_source = "fresh";
        if !params.allow_cached_catalog {
            for snapshot in &snapshots {
                if crate::snapshot_inspection_ipc::prepare_snapshot_filemap_for_browse(
                    &self.config_root,
                    &self.data_root,
                    &settings,
                    &snapshot.snapshot_id,
                )
                .await
                .is_err()
                {
                    return Err(ControlError {
                        code: "snapshot.browse.catalog_refresh_unavailable".to_string(),
                        message: "The remote backup catalog could not be refreshed. You can browse the last cached catalog.".to_string(),
                        retryable: true,
                        details: serde_json::json!({}),
                    });
                }
            }
        } else {
            catalog_source = "cached";
        }
        if params.allow_cached_catalog {
            for snapshot in &snapshots {
                if reader
                    .list_children(&snapshot.snapshot_id, "")
                    .await
                    .is_err()
                {
                    return Err(ControlError {
                        code: "snapshot.browse.catalog_unavailable".to_string(),
                        message:
                            "The cached catalog does not contain all retained snapshot filemaps."
                                .to_string(),
                        retryable: true,
                        details: serde_json::json!({}),
                    });
                }
            }
        }
        let mut unavailable_entries = Vec::new();
        for snapshot in &snapshots {
            if let Ok(paths) = reader.unavailable_entries(&snapshot.snapshot_id).await {
                unavailable_entries.extend(paths.into_iter().map(|path| {
                    if path.is_empty() {
                        snapshot.display_name.clone()
                    } else {
                        format!("{}/{}", snapshot.display_name, path)
                    }
                }));
            }
        }
        let diagnostics_json = Arc::new(
            serde_json::to_vec(&serde_json::json!({
                "entries": unavailable_entries,
                "note": "Some historical metadata cannot be reconstructed from the snapshot."
            }))
            .map(|mut bytes| {
                bytes.push(b'\n');
                bytes
            })
            .unwrap_or_else(|_| {
                b"{\"entries\":[],\"note\":\"Diagnostics unavailable.\"}\n".to_vec()
            }),
        );
        let session_id = format!("browse_{}", Uuid::new_v4().simple());
        let capability = capability_token();
        let volume_name = target
            .label
            .trim()
            .strip_suffix('/')
            .filter(|value| !value.is_empty())
            .unwrap_or("Backup Target")
            .to_string();
        let shutdown = CancellationToken::new();
        let session = Arc::new(BrowseSession {
            id: session_id.clone(),
            target_id: target.id.clone(),
            source_path: target.source_path.clone(),
            volume_name,
            capability: capability.clone(),
            catalog_source: catalog_source.to_string(),
            reader,
            snapshots: Arc::new(Mutex::new(snapshots)),
            metadata_overlay: Arc::new(Mutex::new(HashMap::new())),
            diagnostics_json,
            shutdown: shutdown.clone(),
        });
        let listener = TcpListener::bind(("127.0.0.1", 0))
            .await
            .map_err(io_control_error)?;
        let port = listener.local_addr().map_err(io_control_error)?.port();
        let server_session = session.clone();
        tokio::spawn(async move {
            serve(listener, server_session).await;
        });
        self.sessions
            .lock()
            .await
            .insert(session_id.clone(), session.clone());
        self.target_sessions
            .lock()
            .await
            .insert(target.id, session_id.clone());
        Ok(serde_json::json!({
            "sessionId": session_id,
            "volumeName": session.volume_name,
            "catalogSource": session.catalog_source,
            "mount": { "url": format!("http://127.0.0.1:{port}/{capability}/"), "host": "127.0.0.1", "port": port }
        }))
    }

    async fn status(&self, session_id: &str) -> Result<serde_json::Value, ControlError> {
        let sessions = self.sessions.lock().await;
        let session = sessions.get(session_id).ok_or_else(|| ControlError {
            code: "snapshot.browse.not_found".to_string(),
            message: "Browse session was not found.".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })?;
        Ok(
            serde_json::json!({ "sessionId": session.id, "targetId": session.target_id, "volumeName": session.volume_name, "catalogSource": session.catalog_source, "mountState": "mounted" }),
        )
    }

    async fn unmount(&self, session_id: &str) -> Result<serde_json::Value, ControlError> {
        let _mount_guard = self.mount_lock.lock().await;
        let session = self
            .sessions
            .lock()
            .await
            .remove(session_id)
            .ok_or_else(|| ControlError {
                code: "snapshot.browse.not_found".to_string(),
                message: "Browse session was not found.".to_string(),
                retryable: false,
                details: serde_json::json!({}),
            })?;
        session.shutdown.cancel();
        self.target_sessions.lock().await.remove(&session.target_id);
        Ok(serde_json::json!({ "sessionId": session_id, "unmounted": true }))
    }

    async fn recover(&self) {
        let _mount_guard = self.mount_lock.lock().await;
        let mut sessions = self.sessions.lock().await;
        for session in sessions.values() {
            session.shutdown.cancel();
        }
        sessions.clear();
        self.target_sessions.lock().await.clear();
    }
}

async fn serve(listener: TcpListener, session: Arc<BrowseSession>) {
    let mut methods = DavMethodSet::WEBDAV_RO;
    methods.add(DavMethod::Put);
    let handler = DavHandler::builder()
        .filesystem(Box::new(BrowseDavFs {
            session: session.clone(),
        }))
        .methods(methods)
        .strip_prefix(format!("/{}", session.capability))
        .autoindex(false)
        .build_handler();
    let capability_prefix = format!("/{}/", session.capability);
    let capability_root = capability_prefix.trim_end_matches('/').to_string();
    loop {
        tokio::select! {
            _ = session.shutdown.cancelled() => break,
            result = listener.accept() => {
                let Ok((stream, _)) = result else { continue };
                let handler = handler.clone();
                let connection_capability_prefix = capability_prefix.clone();
                let connection_capability_root = capability_root.clone();
                tokio::spawn(async move {
                    let io = TokioIo::new(stream);
                    let service = service_fn(move |request| {
                        let handler = handler.clone();
                        let capability_prefix = connection_capability_prefix.clone();
                        let capability_root = connection_capability_root.clone();
                        async move {
                            let request_path = request.uri().path();
                            if request_path != capability_root
                                && !request_path.starts_with(&capability_prefix)
                            {
                                let response = Response::builder()
                                    .status(404)
                                    .body(dav_server::body::Body::from("not found"))
                                    .expect("static 404 response");
                                return Ok::<_, Infallible>(response);
                            }
                            if request.method().as_str() == "PUT" {
                                let relative = request_path
                                    .strip_prefix(&capability_prefix)
                                    .unwrap_or_default()
                                    .trim_matches('/');
                                let decoded_relative = decode_path(relative).unwrap_or_default();
                                if !metadata_overlay_path(&decoded_relative) {
                                    let response = Response::builder()
                                        .status(405)
                                        .body(dav_server::body::Body::from("read-only"))
                                        .expect("static 405 response");
                                    return Ok::<_, Infallible>(response);
                                }
                            }
                            Ok::<_, Infallible>(handler.handle(request).await)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
        }
    }
}

#[allow(dead_code)]
async fn handle_connection(
    mut stream: TcpStream,
    session: Arc<BrowseSession>,
) -> std::io::Result<()> {
    let mut request = Vec::with_capacity(4096);
    let mut byte = [0u8; 1];
    while request.len() < MAX_HTTP_REQUEST {
        if stream.read_exact(&mut byte).await.is_err() {
            return Ok(());
        }
        request.push(byte[0]);
        if request.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = String::from_utf8_lossy(&request);
    let mut lines = text.split("\r\n");
    let Some(request_line) = lines.next() else {
        return Ok(());
    };
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or("");
    let raw_path = request_parts.next().unwrap_or("");
    let mut headers = HashMap::new();
    for line in lines {
        if line.is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let content_length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    if content_length > MAX_HTTP_REQUEST {
        stream
            .write_all(&response(413, "text/plain", b"request body too large"))
            .await?;
        return Ok(());
    }
    let mut body = vec![0u8; content_length];
    if content_length > 0 && stream.read_exact(&mut body).await.is_err() {
        return Ok(());
    }
    let response = respond(method, raw_path, &headers, &body, &session).await;
    stream.write_all(&response).await?;
    Ok(())
}

#[allow(dead_code)]
async fn respond(
    method: &str,
    raw_path: &str,
    headers: &HashMap<String, String>,
    body: &[u8],
    session: &BrowseSession,
) -> Vec<u8> {
    let path = match decode_path(raw_path) {
        Ok(path) => path,
        Err(_) => return response(400, "text/plain", b"invalid path"),
    };
    let expected_prefix = format!("/{}/", session.capability);
    if !path.starts_with(&expected_prefix) {
        return response(404, "text/plain", b"not found");
    }
    let relative = path[expected_prefix.len()..].trim_end_matches('/');
    if relative
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return response(404, "text/plain", b"not found");
    }
    match method {
        "OPTIONS" => response_with_headers(
            200,
            "",
            vec![("Allow", "OPTIONS, PROPFIND, GET, HEAD"), ("DAV", "1")],
            &[],
        ),
        "PROPFIND" => {
            propfind(
                relative,
                headers.get("depth").map(String::as_str).unwrap_or("1"),
                session,
            )
            .await
        }
        "GET" | "HEAD" => get_file(method == "HEAD", relative, headers, session).await,
        "PUT" => {
            if metadata_overlay_path(relative) && !real_snapshot_entry(relative, session).await {
                session
                    .metadata_overlay
                    .lock()
                    .await
                    .insert(relative.to_string(), body.to_vec());
                response(201, "text/plain", b"")
            } else {
                response(405, "text/plain", b"read-only")
            }
        }
        "DELETE" | "MOVE" | "COPY" | "MKCOL" | "PROPPATCH" | "LOCK" | "UNLOCK" => {
            response(405, "text/plain", b"read-only")
        }
        _ => response(405, "text/plain", b"method not allowed"),
    }
}

#[allow(dead_code)]
async fn propfind(relative: &str, depth: &str, session: &BrowseSession) -> Vec<u8> {
    let mut resources = Vec::new();
    if relative.is_empty() {
        resources.push((String::new(), "dir".to_string(), 0u64, 0i64));
        if depth != "0" {
            let snapshots = refresh_snapshots(session).await;
            for snapshot in snapshots {
                resources.push((snapshot.display_name, "dir".to_string(), 0, 0));
            }
            resources.push((DIAGNOSTICS_DIRECTORY.to_string(), "dir".to_string(), 0, 0));
            resources.extend(
                session
                    .metadata_overlay
                    .lock()
                    .await
                    .keys()
                    .filter(|path| !path.contains('/'))
                    .map(|path| (path.clone(), "file".to_string(), 0, 0)),
            );
        }
    } else if let Some((snapshot, snapshot_path)) = snapshot_path(relative, session).await {
        if let Ok(Some(entry)) = session
            .reader
            .entry(&snapshot.snapshot_id, &snapshot_path)
            .await
        {
            resources.push((relative.to_string(), entry.kind, entry.size, entry.mtime_ms));
            if depth != "0"
                && let Ok(children) = session
                    .reader
                    .list_children(&snapshot.snapshot_id, &snapshot_path)
                    .await
            {
                resources.extend(children.into_iter().map(|entry| {
                    (
                        relative_join(relative, &entry.name),
                        entry.kind,
                        entry.size,
                        entry.mtime_ms,
                    )
                }));
            }
        } else if session.metadata_overlay.lock().await.contains_key(relative) {
            resources.push((relative.to_string(), "file".to_string(), 0, 0));
        }
    } else if relative == DIAGNOSTICS_DIRECTORY
        || relative == format!("{DIAGNOSTICS_DIRECTORY}/{UNAVAILABLE_ENTRIES_FILE}")
    {
        resources.push((
            relative.to_string(),
            if relative.ends_with(".json") {
                "file".to_string()
            } else {
                "dir".to_string()
            },
            if relative.ends_with(".json") {
                session.diagnostics_json.len() as u64
            } else {
                0
            },
            0,
        ));
    }
    if resources.is_empty() {
        return response(404, "text/plain", b"not found");
    }
    let mut xml =
        String::from("<?xml version=\"1.0\" encoding=\"utf-8\"?><D:multistatus xmlns:D=\"DAV:\">");
    for (path, kind, size, mtime) in resources {
        let collection = kind == "dir";
        let encoded_path = uri_path(&path);
        let href = if encoded_path.is_empty() {
            format!("/{}/", session.capability)
        } else if collection {
            format!("/{}/{}/", session.capability, encoded_path)
        } else {
            format!("/{}/{}", session.capability, encoded_path)
        };
        xml.push_str(&format!("<D:response><D:href>{}</D:href><D:propstat><D:prop><D:resourcetype>{}</D:resourcetype><D:getcontentlength>{size}</D:getcontentlength><D:getlastmodified>{}</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>", xml_escape(&href), if collection { "<D:collection/>" } else { "" }, http_date(mtime)));
    }
    xml.push_str("</D:multistatus>");
    response(207, "application/xml; charset=utf-8", xml.as_bytes())
}

#[allow(dead_code)]
async fn get_file(
    head: bool,
    relative: &str,
    headers: &HashMap<String, String>,
    session: &BrowseSession,
) -> Vec<u8> {
    if let Some(bytes) = session.metadata_overlay.lock().await.get(relative).cloned() {
        return response_with_headers(
            200,
            "application/octet-stream",
            vec![("Content-Length", &bytes.len().to_string())],
            if head { &[] } else { &bytes },
        );
    }
    if relative == format!("{DIAGNOSTICS_DIRECTORY}/{UNAVAILABLE_ENTRIES_FILE}") {
        let bytes = session.diagnostics_json.as_slice();
        return response_with_headers(
            200,
            "application/json",
            vec![("Content-Length", &bytes.len().to_string())],
            if head { &[] } else { bytes },
        );
    }
    let Some((snapshot, snapshot_path)) = snapshot_path(relative, session).await else {
        return response(404, "text/plain", b"not found");
    };
    let Some(entry) = session
        .reader
        .entry(&snapshot.snapshot_id, &snapshot_path)
        .await
        .ok()
        .flatten()
    else {
        return response(404, "text/plain", b"not found");
    };
    if entry.kind != "file" {
        return response(404, "text/plain", b"not a file");
    }
    let (start, len, partial) = parse_range(headers.get("range").map(String::as_str), entry.size);
    if start > entry.size {
        return response(416, "text/plain", b"range not satisfiable");
    }
    let Ok((_entry, bytes)) = session
        .reader
        .read_range(&snapshot.snapshot_id, &snapshot_path, start, len)
        .await
    else {
        return response(503, "text/plain", b"snapshot data unavailable");
    };
    let status = if partial { 206 } else { 200 };
    let mut extra = vec![
        ("Accept-Ranges", "bytes".to_string()),
        ("Content-Length", bytes.len().to_string()),
    ];
    if partial {
        extra.push((
            "Content-Range",
            format!(
                "bytes {}-{}/{}",
                start,
                start + bytes.len().saturating_sub(1) as u64,
                entry.size
            ),
        ));
    }
    response_with_headers(
        status,
        "application/octet-stream",
        extra
            .iter()
            .map(|(k, v)| (*k, v.as_str()))
            .collect::<Vec<_>>(),
        if head { &[] } else { &bytes },
    )
}

async fn snapshot_path(
    relative: &str,
    session: &BrowseSession,
) -> Option<(BrowseSnapshot, String)> {
    let (name, rest) = relative.split_once('/').unwrap_or((relative, ""));
    let snapshots = refresh_snapshots(session).await;
    let snapshot = snapshots
        .into_iter()
        .find(|snapshot| snapshot.display_name == name)?;
    Some((snapshot, rest.to_string()))
}

async fn refresh_snapshots(session: &BrowseSession) -> Vec<BrowseSnapshot> {
    match session.reader.list_snapshots(&session.source_path).await {
        Ok(snapshots) => {
            *session.snapshots.lock().await = snapshots.clone();
            snapshots
        }
        Err(_) => {
            session.snapshots.lock().await.clear();
            Vec::new()
        }
    }
}

fn metadata_overlay_path(relative: &str) -> bool {
    let name = relative.rsplit('/').next().unwrap_or(relative);
    name == ".DS_Store" || name.starts_with("._")
}

async fn real_snapshot_entry(relative: &str, session: &BrowseSession) -> bool {
    let Some((snapshot, snapshot_path)) = snapshot_path(relative, session).await else {
        return false;
    };
    session
        .reader
        .entry(&snapshot.snapshot_id, &snapshot_path)
        .await
        .ok()
        .flatten()
        .is_some()
}

#[allow(dead_code)]
fn parse_range(value: Option<&str>, size: u64) -> (u64, Option<u64>, bool) {
    let Some(value) = value.and_then(|value| value.strip_prefix("bytes=")) else {
        return (0, None, false);
    };
    let Some((start, end)) = value.split_once('-') else {
        return (size.saturating_add(1), None, true);
    };
    if start.is_empty() {
        let Ok(suffix_len) = end.parse::<u64>() else {
            return (size.saturating_add(1), None, true);
        };
        if suffix_len == 0 || size == 0 {
            return (size.saturating_add(1), None, true);
        }
        let len = suffix_len.min(size);
        return (size - len, Some(len), true);
    }
    let Ok(start) = start.parse::<u64>() else {
        return (size.saturating_add(1), None, true);
    };
    if start >= size {
        return (size.saturating_add(1), None, true);
    }
    let len = if end.is_empty() {
        None
    } else {
        let Ok(end) = end.parse::<u64>() else {
            return (size.saturating_add(1), None, true);
        };
        if end < start {
            return (size.saturating_add(1), None, true);
        }
        Some(end.min(size - 1) - start + 1)
    };
    (start, len, true)
}

#[allow(dead_code)]
fn response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    response_with_headers(
        status,
        content_type,
        vec![("Content-Length", &body.len().to_string())],
        body,
    )
}

#[allow(dead_code)]
fn response_with_headers(
    status: u16,
    content_type: &str,
    headers: Vec<(&str, &str)>,
    body: &[u8],
) -> Vec<u8> {
    let phrase = match status {
        200 => "OK",
        201 => "Created",
        206 => "Partial Content",
        207 => "Multi-Status",
        400 => "Bad Request",
        404 => "Not Found",
        405 => "Method Not Allowed",
        416 => "Range Not Satisfiable",
        503 => "Service Unavailable",
        _ => "Error",
    };
    let mut out = format!(
        "HTTP/1.1 {status} {phrase}\r\nContent-Type: {content_type}\r\nConnection: close\r\n"
    );
    for (key, value) in headers {
        out.push_str(&format!("{key}: {value}\r\n"));
    }
    out.push_str("\r\n");
    let mut out = out.into_bytes();
    out.extend_from_slice(body);
    out
}

#[allow(dead_code)]
fn relative_join(parent: &str, name: &str) -> String {
    if parent.is_empty() {
        name.to_string()
    } else {
        format!("{parent}/{name}")
    }
}
#[allow(dead_code)]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[allow(dead_code)]
fn uri_path(value: &str) -> String {
    value
        .split('/')
        .map(|segment| {
            let mut out = String::new();
            for byte in segment.as_bytes() {
                if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                    out.push(*byte as char);
                } else {
                    out.push_str(&format!("%{byte:02X}"));
                }
            }
            out
        })
        .collect::<Vec<_>>()
        .join("/")
}

#[allow(dead_code)]
fn http_date(mtime_ms: i64) -> String {
    DateTime::<Utc>::from_timestamp_millis(mtime_ms.max(0))
        .unwrap_or(DateTime::<Utc>::UNIX_EPOCH)
        .format("%a, %d %b %Y %H:%M:%S GMT")
        .to_string()
}

#[allow(dead_code)]
fn decode_path(value: &str) -> Result<String, ()> {
    let mut bytes = Vec::with_capacity(value.len());
    let raw = value.as_bytes();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'%' {
            if index + 2 >= raw.len() {
                return Err(());
            }
            let hi = hex(raw[index + 1]).ok_or(())?;
            let lo = hex(raw[index + 2]).ok_or(())?;
            bytes.push(hi * 16 + lo);
            index += 3;
        } else {
            bytes.push(raw[index]);
            index += 1;
        }
    }
    String::from_utf8(bytes).map_err(|_| ())
}
#[allow(dead_code)]
fn hex(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}
fn capability_token() -> String {
    let mut bytes = [0u8; 32];
    getrandom(&mut bytes).expect("OS random source unavailable");
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

async fn find_target_index(
    data_root: &Path,
    target: &Target,
) -> Result<(PathBuf, PathBuf), ControlError> {
    let index_dir = data_root.join("index");
    let endpoint_db = index_dir.join(format!("index.{}.sqlite", target.endpoint_id));
    Ok((
        endpoint_db,
        index_dir.join("filemaps").join(&target.endpoint_id),
    ))
}

async fn connect_storage(
    config_root: &Path,
    data_root: &Path,
    settings: &SettingsV2,
    target: &Target,
) -> Result<(TelegramMtProtoStorage, [u8; 32]), ControlError> {
    let endpoint = settings
        .telegram_endpoints
        .iter()
        .find(|endpoint| endpoint.id == target.endpoint_id)
        .ok_or_else(|| {
            ControlError::invalid_request("target endpoint was not found", serde_json::json!({}))
        })?;
    let vault_key = crate::load_or_create_vault_key().map_err(|error| ControlError {
        code: "secrets.vault_unavailable".to_string(),
        message: error.to_string(),
        retryable: true,
        details: serde_json::json!({}),
    })?;
    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    let secrets = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
        .map_err(|error| ControlError {
            code: "secrets.store_failed".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })?;
    let get = |key: &str, code: &str| {
        secrets
            .get(key)
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .ok_or_else(|| ControlError {
                code: code.to_string(),
                message: "Required backup secret is unavailable.".to_string(),
                retryable: false,
                details: serde_json::json!({}),
            })
    };
    let bot_token = get(&endpoint.bot_token_key, "telegram.unauthorized")?;
    let api_hash = get(
        &settings.telegram.mtproto.api_hash_key,
        "telegram.mtproto.missing_api_hash",
    )?;
    let master_key =
        crate::decode_base64_32(&get(crate::MASTER_KEY_KEY, "secrets.master_key_missing")?)
            .map_err(|error| ControlError {
                code: "secrets.master_key_invalid".to_string(),
                message: error.to_string(),
                retryable: false,
                details: serde_json::json!({}),
            })?;
    let session = secrets
        .get(&endpoint.mtproto.session_key)
        .filter(|value| !value.trim().is_empty())
        .map(|value| base64::engine::general_purpose::STANDARD.decode(value.as_bytes()))
        .transpose()
        .map_err(|error| ControlError {
            code: "telegram.mtproto.session_invalid".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })?;
    let cache_dir = data_root.join("cache").join("mtproto").join(&endpoint.id);
    std::fs::create_dir_all(&cache_dir).map_err(io_control_error)?;
    let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
        provider: endpoint_provider(&endpoint.id),
        api_id: settings.telegram.mtproto.api_id,
        api_hash,
        bot_token,
        chat_id: endpoint.chat_id.clone(),
        session,
        cache_dir,
        min_delay_ms: Some(endpoint.rate_limit.min_delay_ms as u64),
        max_concurrent_uploads: Some(endpoint.rate_limit.max_concurrent_uploads as usize),
        helper_path: None,
    })
    .await
    .map_err(core_error)?;
    Ok((storage, master_key))
}

fn decode_params<T: serde::de::DeserializeOwned>(
    value: &serde_json::Value,
) -> Result<T, ControlError> {
    serde_json::from_value(value.clone()).map_err(|error| {
        ControlError::invalid_request(
            "invalid params",
            serde_json::json!({ "error": error.to_string() }),
        )
    })
}
fn core_error(error: televy_backup_core::Error) -> ControlError {
    ControlError {
        code: error.code().to_string(),
        message: error.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    }
}
fn io_control_error(error: std::io::Error) -> ControlError {
    ControlError {
        code: "snapshot.browse.io".to_string(),
        message: error.to_string(),
        retryable: true,
        details: serde_json::json!({}),
    }
}

fn sanitize_mount_error(error: ControlError) -> ControlError {
    ControlError {
        code: error.code,
        message: "Backup storage is unavailable for snapshot browsing.".to_string(),
        retryable: error.retryable,
        details: serde_json::json!({}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parser_supports_open_ended_and_rejects_out_of_bounds() {
        assert_eq!(parse_range(None, 10), (0, None, false));
        assert_eq!(parse_range(Some("bytes=2-5"), 10), (2, Some(4), true));
        assert_eq!(parse_range(Some("bytes=2-"), 10), (2, None, true));
        assert_eq!(parse_range(Some("bytes=10-"), 10), (11, None, true));
        assert_eq!(parse_range(Some("bytes=-3"), 10), (7, Some(3), true));
        assert_eq!(parse_range(Some("bytes=8-3"), 10), (11, None, true));
    }

    #[test]
    fn binary_response_does_not_round_trip_through_utf8() {
        let body = [0, 0xff, 1, 0xfe];
        let response = response(200, "application/octet-stream", &body);
        assert!(response.ends_with(&body));
    }

    #[test]
    fn metadata_overlay_is_limited_to_finder_sidecars() {
        assert!(metadata_overlay_path(".DS_Store"));
        assert!(metadata_overlay_path("snapshot/._file"));
        assert!(!metadata_overlay_path("snapshot/real-file"));
    }

    #[test]
    fn capability_is_256_bits_and_url_safe() {
        let token = capability_token();
        assert_eq!(
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .decode(&token)
                .unwrap()
                .len(),
            32
        );
        assert!(!token.contains('='));
    }

    #[test]
    fn webdav_paths_are_percent_encoded() {
        assert_eq!(
            uri_path("2026-09-11 14-05-37 [abc]/报告.txt"),
            "2026-09-11%2014-05-37%20%5Babc%5D/%E6%8A%A5%E5%91%8A.txt"
        );
        assert_eq!(http_date(0), "Thu, 01 Jan 1970 00:00:00 GMT");
    }
}
