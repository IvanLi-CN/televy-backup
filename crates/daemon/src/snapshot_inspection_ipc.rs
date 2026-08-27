use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use base64::Engine;
use sqlx::Row;
use tokio::sync::{Mutex, RwLock};

use televy_backup_core::control::{
    ControlError, ControlRequest, ControlResponse, SnapshotInspectBlocksParams,
    SnapshotInspectFilesParams, SnapshotInspectSummaryParams,
};
use televy_backup_core::snapshot_inspection::{
    BlockInspectionRequest, FileInspectionRequest, FilePresentation, FileScope,
    SnapshotInspectionError, SnapshotInspectionSession, SnapshotInspector,
};
use televy_backup_core::{TelegramMtProtoStorage, TelegramMtProtoStorageConfig};

const MAX_PREPARED_SNAPSHOTS: usize = 2;

type Settings = televy_backup_core::config::SettingsV2;

pub(crate) struct SnapshotInspectionService {
    config_root: PathBuf,
    data_root: PathBuf,
    settings: Arc<RwLock<Settings>>,
    cache: Mutex<PreparedSnapshots>,
}

struct PreparedSnapshots {
    entries: HashMap<String, Arc<SnapshotInspectionSession>>,
    least_recently_used: VecDeque<String>,
}

impl SnapshotInspectionService {
    pub(crate) fn new(
        config_root: PathBuf,
        data_root: PathBuf,
        settings: Arc<RwLock<Settings>>,
    ) -> Self {
        Self {
            config_root,
            data_root,
            settings,
            cache: Mutex::new(PreparedSnapshots {
                entries: HashMap::new(),
                least_recently_used: VecDeque::new(),
            }),
        }
    }

    pub(crate) async fn handle(&self, request: &ControlRequest) -> ControlResponse {
        let result = match request.method.as_str() {
            "snapshot.inspect.summary" => self.summary(request).await,
            "snapshot.inspect.files" => self.files(request).await,
            "snapshot.inspect.blocks" => self.blocks(request).await,
            _ => {
                return ControlResponse::err(
                    request.id.clone(),
                    ControlError::method_not_found(
                        "method not found",
                        serde_json::json!({ "method": request.method }),
                    ),
                );
            }
        };
        match result {
            Ok(result) => ControlResponse::ok(request.id.clone(), result),
            Err(error) => ControlResponse::err(request.id.clone(), error),
        }
    }

    pub(crate) async fn clear(&self) {
        let mut cache = self.cache.lock().await;
        cache.entries.clear();
        cache.least_recently_used.clear();
    }

    async fn summary(&self, request: &ControlRequest) -> Result<serde_json::Value, ControlError> {
        let params: SnapshotInspectSummaryParams = decode_params(&request.params)?;
        let session = self.session_for(&params.snapshot_id).await?;
        serde_json::to_value(session.summary()).map_err(serialization_error)
    }

    async fn files(&self, request: &ControlRequest) -> Result<serde_json::Value, ControlError> {
        let params: SnapshotInspectFilesParams = decode_params(&request.params)?;
        let presentation = match params.presentation.as_str() {
            "tree" => FilePresentation::Tree,
            "list" => FilePresentation::List,
            _ => {
                return Err(ControlError::invalid_request(
                    "presentation must be tree or list",
                    serde_json::json!({}),
                ));
            }
        };
        let scope = match params.scope.as_str() {
            "all" => FileScope::All,
            "changes" => FileScope::Changes,
            _ => {
                return Err(ControlError::invalid_request(
                    "scope must be all or changes",
                    serde_json::json!({}),
                ));
            }
        };
        let snapshot_id = params.snapshot_id.clone();
        let session = self.session_for(&snapshot_id).await?;
        let page = session
            .files(FileInspectionRequest {
                snapshot_id,
                presentation,
                scope,
                parent: params.parent,
                query: params.query,
                cursor: params.cursor,
                limit: params.limit,
            })
            .await
            .map_err(map_inspection_error)?;
        serde_json::to_value(page).map_err(serialization_error)
    }

    async fn blocks(&self, request: &ControlRequest) -> Result<serde_json::Value, ControlError> {
        let params: SnapshotInspectBlocksParams = decode_params(&request.params)?;
        let snapshot_id = params.snapshot_id.clone();
        let session = self.session_for(&snapshot_id).await?;
        let page = session
            .blocks(BlockInspectionRequest {
                snapshot_id,
                changes_only: params.changes_only,
                query: params.query,
                cursor: params.cursor,
                limit: params.limit,
            })
            .await
            .map_err(map_inspection_error)?;
        serde_json::to_value(page).map_err(serialization_error)
    }

    async fn session_for(
        &self,
        snapshot_id: &str,
    ) -> Result<Arc<SnapshotInspectionSession>, ControlError> {
        // Preparing a difference is intentionally serialized. It avoids duplicating a large
        // direct-baseline scan when the UI issues concurrent detail requests for one snapshot.
        let mut cache = self.cache.lock().await;
        if let Some(session) = cache.entries.get(snapshot_id).cloned() {
            touch(&mut cache.least_recently_used, snapshot_id);
            return Ok(session);
        }

        let settings = self.settings.read().await.clone();
        let inspector =
            snapshot_inspector_for(&self.config_root, &self.data_root, &settings, snapshot_id)
                .await?;
        let session = Arc::new(
            inspector
                .prepare(snapshot_id)
                .await
                .map_err(map_inspection_error)?,
        );
        cache
            .entries
            .insert(snapshot_id.to_string(), session.clone());
        touch(&mut cache.least_recently_used, snapshot_id);
        while cache.entries.len() > MAX_PREPARED_SNAPSHOTS {
            if let Some(expired) = cache.least_recently_used.pop_front() {
                cache.entries.remove(&expired);
            }
        }
        Ok(session)
    }
}

fn touch(least_recently_used: &mut VecDeque<String>, snapshot_id: &str) {
    least_recently_used.retain(|entry| entry != snapshot_id);
    least_recently_used.push_back(snapshot_id.to_string());
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

fn serialization_error(error: serde_json::Error) -> ControlError {
    ControlError::unavailable(
        "snapshot inspection serialization failed",
        serde_json::json!({ "error": error.to_string() }),
    )
}

async fn snapshot_inspector_for(
    config_root: &Path,
    data_root: &Path,
    settings: &Settings,
    snapshot_id: &str,
) -> Result<SnapshotInspector, ControlError> {
    let endpoint_db_path = find_snapshot_endpoint_db(data_root, snapshot_id).await?;
    let mut filemap_dir = endpoint_filemap_dir_for_db(data_root, &endpoint_db_path);
    let provider = snapshot_provider_for(&endpoint_db_path, snapshot_id).await?;
    if let Some(endpoint_id) = endpoint_id_from_provider(provider.as_deref())? {
        filemap_dir = endpoint_filemap_dir(data_root, endpoint_id);
    }
    ensure_snapshot_filemap(
        config_root,
        data_root,
        settings,
        &endpoint_db_path,
        &filemap_dir,
        snapshot_id,
        provider.as_deref(),
    )
    .await?;

    let endpoint_pool = televy_backup_core::index_db::open_existing_index_db(&endpoint_db_path)
        .await
        .map_err(core_error)?;
    let base_snapshot_id =
        sqlx::query("SELECT base_snapshot_id FROM snapshots WHERE snapshot_id = ? LIMIT 1")
            .bind(snapshot_id)
            .fetch_one(&endpoint_pool)
            .await
            .map_err(sql_error)?
            .get::<Option<String>, _>("base_snapshot_id");
    if let Some(base_snapshot_id) = base_snapshot_id {
        let base_retained = sqlx::query("SELECT 1 FROM snapshots WHERE snapshot_id = ? LIMIT 1")
            .bind(&base_snapshot_id)
            .fetch_optional(&endpoint_pool)
            .await
            .map_err(sql_error)?
            .is_some();
        if base_retained {
            let base_provider = snapshot_provider_for(&endpoint_db_path, &base_snapshot_id).await?;
            ensure_snapshot_filemap(
                config_root,
                data_root,
                settings,
                &endpoint_db_path,
                &filemap_dir,
                &base_snapshot_id,
                base_provider.as_deref(),
            )
            .await?;
        }
    }
    Ok(SnapshotInspector::new(endpoint_db_path, filemap_dir))
}

async fn find_snapshot_endpoint_db(
    data_root: &Path,
    snapshot_id: &str,
) -> Result<PathBuf, ControlError> {
    for db_path in list_index_db_paths_for_read(data_root)? {
        let pool = televy_backup_core::index_db::open_existing_index_db(&db_path)
            .await
            .map_err(core_error)?;
        let exists = sqlx::query("SELECT 1 FROM snapshots WHERE snapshot_id = ? LIMIT 1")
            .bind(snapshot_id)
            .fetch_optional(&pool)
            .await
            .map_err(sql_error)?
            .is_some();
        if exists {
            return Ok(db_path);
        }
    }
    Err(ControlError {
        code: "snapshot.not_found".to_string(),
        message: format!("snapshot is not retained locally: {snapshot_id}"),
        retryable: false,
        details: serde_json::json!({ "snapshotId": snapshot_id }),
    })
}

fn list_index_db_paths_for_read(data_root: &Path) -> Result<Vec<PathBuf>, ControlError> {
    let index_dir = data_root.join("index");
    let mut paths = Vec::new();
    match std::fs::read_dir(&index_dir) {
        Ok(entries) => {
            for entry in entries {
                let path = entry.map_err(io_error)?.path();
                let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                    continue;
                };
                if name != "index.sqlite"
                    && name.starts_with("index.")
                    && name.ends_with(".sqlite")
                    && path.is_file()
                {
                    paths.push(path);
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(io_error(error)),
    }
    paths.sort();
    if paths.is_empty() {
        let legacy = index_dir.join("index.sqlite");
        if legacy.is_file() {
            paths.push(legacy);
        }
    }
    Ok(paths)
}

fn endpoint_filemap_dir_for_db(data_root: &Path, db_path: &Path) -> PathBuf {
    db_path
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("index."))
        .and_then(|name| name.strip_suffix(".sqlite"))
        .map(|endpoint_id| endpoint_filemap_dir(data_root, endpoint_id))
        .unwrap_or_else(|| data_root.join("index").join("filemaps"))
}

fn endpoint_filemap_dir(data_root: &Path, endpoint_id: &str) -> PathBuf {
    data_root.join("index").join("filemaps").join(endpoint_id)
}

async fn snapshot_provider_for(
    endpoint_db_path: &Path,
    snapshot_id: &str,
) -> Result<Option<String>, ControlError> {
    let pool = televy_backup_core::index_db::open_existing_index_db(endpoint_db_path)
        .await
        .map_err(core_error)?;
    sqlx::query("SELECT provider FROM remote_indexes WHERE snapshot_id = ? LIMIT 1")
        .bind(snapshot_id)
        .fetch_optional(&pool)
        .await
        .map_err(sql_error)
        .map(|row| row.map(|row| row.get("provider")))
}

fn endpoint_id_from_provider(provider: Option<&str>) -> Result<Option<&str>, ControlError> {
    match provider {
        None | Some("telegram.mtproto") => Ok(None),
        Some(provider) => provider
            .strip_prefix("telegram.mtproto/")
            .map(Some)
            .ok_or_else(|| ControlError {
                code: "snapshot.filemap_unavailable".to_string(),
                message: format!("unsupported snapshot provider: {provider}"),
                retryable: false,
                details: serde_json::json!({}),
            }),
    }
}

async fn ensure_snapshot_filemap(
    config_root: &Path,
    data_root: &Path,
    settings: &Settings,
    endpoint_db_path: &Path,
    filemap_dir: &Path,
    snapshot_id: &str,
    provider_hint: Option<&str>,
) -> Result<(), ControlError> {
    let cached_filemap = filemap_dir.join(format!("{snapshot_id}.sqlite"));
    if cached_filemap.is_file()
        || endpoint_db_path.file_name().and_then(|name| name.to_str()) == Some("index.sqlite")
        || endpoint_has_snapshot_files(endpoint_db_path, snapshot_id).await?
    {
        return Ok(());
    }

    let pool = televy_backup_core::index_db::open_existing_index_db(endpoint_db_path)
        .await
        .map_err(core_error)?;
    let remote_index = sqlx::query(
        "SELECT provider, manifest_object_id FROM remote_indexes WHERE snapshot_id = ? LIMIT 1",
    )
    .bind(snapshot_id)
    .fetch_optional(&pool)
    .await
    .map_err(sql_error)?
    .ok_or_else(|| ControlError {
        code: "snapshot.filemap_unavailable".to_string(),
        message: format!("retained snapshot has no filemap index pointer: {snapshot_id}"),
        retryable: false,
        details: serde_json::json!({ "snapshotId": snapshot_id }),
    })?;
    let provider: String = remote_index.get("provider");
    let manifest_object_id: String = remote_index.get("manifest_object_id");
    let endpoint_id = endpoint_id_from_provider(provider_hint.or(Some(provider.as_str())))?;
    let endpoint = select_endpoint(settings, endpoint_id)?;
    if settings.telegram.mtproto.api_id <= 0 {
        return Err(ControlError {
            code: "config.invalid".to_string(),
            message: "telegram.mtproto.api_id must be > 0".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }

    let vault_key = crate::load_or_create_vault_key().map_err(|error| ControlError {
        code: "secrets.vault_unavailable".to_string(),
        message: error.to_string(),
        retryable: true,
        details: serde_json::json!({}),
    })?;
    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    let mut secrets = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
        .map_err(|error| ControlError {
            code: "secrets.store_failed".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })?;
    let bot_token = secret(
        &secrets,
        &endpoint.bot_token_key,
        "telegram.unauthorized",
        "bot token missing",
    )?;
    let api_hash = secret(
        &secrets,
        &settings.telegram.mtproto.api_hash_key,
        "telegram.mtproto.missing_api_hash",
        "mtproto api_hash missing",
    )?;
    let master_key_b64 = secret(
        &secrets,
        crate::MASTER_KEY_KEY,
        "secrets.master_key_missing",
        "master key missing",
    )?;
    let master_key = crate::decode_base64_32(&master_key_b64).map_err(|error| ControlError {
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

    std::fs::create_dir_all(filemap_dir).map_err(io_error)?;
    let cache_dir = data_root.join("cache").join("mtproto").join(&endpoint.id);
    std::fs::create_dir_all(&cache_dir).map_err(io_error)?;
    let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
        provider: provider.clone(),
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
    televy_backup_core::remote_index_db::download_and_write_index_db_atomic(
        &storage,
        snapshot_id,
        &manifest_object_id,
        &master_key,
        &cached_filemap,
        None,
        Some(&provider),
        None,
    )
    .await
    .map_err(core_error)?;
    if let Some(session) = storage.session_bytes() {
        secrets.set(
            &endpoint.mtproto.session_key,
            base64::engine::general_purpose::STANDARD.encode(session),
        );
        televy_backup_core::secrets::save_secrets_store(&secrets_path, &vault_key, &secrets)
            .map_err(|error| ControlError {
                code: "secrets.store_failed".to_string(),
                message: error.to_string(),
                retryable: false,
                details: serde_json::json!({}),
            })?;
    }
    Ok(())
}

async fn endpoint_has_snapshot_files(
    endpoint_db_path: &Path,
    snapshot_id: &str,
) -> Result<bool, ControlError> {
    let pool = televy_backup_core::index_db::open_existing_index_db(endpoint_db_path)
        .await
        .map_err(core_error)?;
    sqlx::query("SELECT 1 FROM files WHERE snapshot_id = ? LIMIT 1")
        .bind(snapshot_id)
        .fetch_optional(&pool)
        .await
        .map_err(sql_error)
        .map(|row| row.is_some())
}

fn select_endpoint<'a>(
    settings: &'a Settings,
    endpoint_id: Option<&str>,
) -> Result<&'a televy_backup_core::config::TelegramEndpoint, ControlError> {
    if let Some(endpoint_id) = endpoint_id {
        return settings
            .telegram_endpoints
            .iter()
            .find(|endpoint| endpoint.id == endpoint_id)
            .ok_or_else(|| ControlError {
                code: "config.invalid".to_string(),
                message: format!("unknown endpoint_id: {endpoint_id}"),
                retryable: false,
                details: serde_json::json!({}),
            });
    }
    if settings.telegram_endpoints.len() == 1 {
        return Ok(&settings.telegram_endpoints[0]);
    }
    settings
        .telegram_endpoints
        .iter()
        .find(|endpoint| endpoint.id == "default")
        .ok_or_else(|| ControlError {
            code: "config.invalid".to_string(),
            message: "multiple endpoints configured; snapshot provider did not identify one"
                .to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })
}

fn secret(
    secrets: &televy_backup_core::secrets::SecretsStore,
    key: &str,
    code: &str,
    message: &str,
) -> Result<String, ControlError> {
    secrets
        .get(key)
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .ok_or_else(|| ControlError {
            code: code.to_string(),
            message: message.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })
}

fn map_inspection_error(error: SnapshotInspectionError) -> ControlError {
    match error {
        SnapshotInspectionError::SnapshotNotFound { snapshot_id } => ControlError {
            code: "snapshot.not_found".to_string(),
            message: format!("snapshot was not found: {snapshot_id}"),
            retryable: false,
            details: serde_json::json!({ "snapshotId": snapshot_id }),
        },
        SnapshotInspectionError::SnapshotNotRetained { snapshot_id } => ControlError {
            code: "snapshot.not_retained".to_string(),
            message: format!("snapshot is no longer retained: {snapshot_id}"),
            retryable: false,
            details: serde_json::json!({ "snapshotId": snapshot_id }),
        },
        SnapshotInspectionError::FilemapUnavailable {
            snapshot_id,
            message,
        } => ControlError {
            code: "snapshot.filemap_unavailable".to_string(),
            message,
            retryable: true,
            details: serde_json::json!({ "snapshotId": snapshot_id }),
        },
        SnapshotInspectionError::BaselineUnavailable { snapshot_id } => ControlError {
            code: "snapshot.baseline_unavailable".to_string(),
            message: format!("the direct baseline is unavailable: {snapshot_id}"),
            retryable: false,
            details: serde_json::json!({ "snapshotId": snapshot_id }),
        },
        SnapshotInspectionError::InvalidArgument { message } => {
            ControlError::invalid_request(message, serde_json::json!({}))
        }
        SnapshotInspectionError::InvalidCursor { message } => ControlError {
            code: "snapshot.inspect.invalid_cursor".to_string(),
            message,
            retryable: false,
            details: serde_json::json!({}),
        },
        SnapshotInspectionError::Core(error) => core_error(error),
    }
}

fn core_error(error: televy_backup_core::Error) -> ControlError {
    let retryable = matches!(error, televy_backup_core::Error::Telegram { .. });
    ControlError {
        code: if retryable {
            "telegram.unavailable".to_string()
        } else {
            "snapshot.inspect.failed".to_string()
        },
        message: error.to_string(),
        retryable,
        details: serde_json::json!({}),
    }
}

fn sql_error(error: sqlx::Error) -> ControlError {
    ControlError::unavailable(
        "snapshot inspection database failed",
        serde_json::json!({ "error": error.to_string() }),
    )
}

fn io_error(error: std::io::Error) -> ControlError {
    ControlError::unavailable(
        "snapshot inspection local storage failed",
        serde_json::json!({ "error": error.to_string() }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::UnixStream;

    async fn request_over_control_socket(
        socket_path: &Path,
        request: ControlRequest,
    ) -> ControlResponse {
        let stream = UnixStream::connect(socket_path).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = tokio::io::BufReader::new(reader).lines();
        writer
            .write_all(format!("{}\n", serde_json::to_string(&request).unwrap()).as_bytes())
            .await
            .unwrap();
        writer.flush().await.unwrap();
        serde_json::from_str(&reader.next_line().await.unwrap().unwrap()).unwrap()
    }

    async fn insert_snapshot(
        pool: &sqlx::SqlitePool,
        snapshot_id: &str,
        base_snapshot_id: Option<&str>,
    ) {
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, '2026-08-27T00:00:00Z', '/source', 'Project', ?)",
        )
        .bind(snapshot_id)
        .bind(base_snapshot_id)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn insert_file(
        pool: &sqlx::SqlitePool,
        file_id: &str,
        snapshot_id: &str,
        path: &str,
        kind: &str,
        size: i64,
        mtime_ms: i64,
    ) {
        sqlx::query(
            "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES (?, ?, ?, ?, ?, 33188, ?)",
        )
        .bind(file_id)
        .bind(snapshot_id)
        .bind(path)
        .bind(size)
        .bind(mtime_ms)
        .bind(kind)
        .execute(pool)
        .await
        .unwrap();
    }

    async fn service_fixture() -> (tempfile::TempDir, SnapshotInspectionService, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let data_root = temp.path().join("data");
        let endpoint_path = data_root.join("index").join("index.ep1.sqlite");
        std::fs::create_dir_all(endpoint_path.parent().unwrap()).unwrap();
        let endpoint = televy_backup_core::index_db::open_index_db(&endpoint_path)
            .await
            .unwrap();
        insert_snapshot(&endpoint, "base", None).await;
        insert_snapshot(&endpoint, "current", Some("base")).await;
        drop(endpoint);

        let filemap_dir = data_root.join("index").join("filemaps").join("ep1");
        std::fs::create_dir_all(&filemap_dir).unwrap();
        let base_path = filemap_dir.join("base.sqlite");
        let base = televy_backup_core::index_db::open_index_db(&base_path)
            .await
            .unwrap();
        insert_snapshot(&base, "base", None).await;
        insert_file(&base, "base-docs", "base", "docs", "dir", 0, 1).await;
        insert_file(&base, "base-old", "base", "docs/old.txt", "file", 1, 1).await;
        drop(base);

        let current_path = filemap_dir.join("current.sqlite");
        let current = televy_backup_core::index_db::open_index_db(&current_path)
            .await
            .unwrap();
        insert_snapshot(&current, "current", Some("base")).await;
        insert_file(&current, "current-docs", "current", "docs", "dir", 0, 1).await;
        insert_file(
            &current,
            "current-new",
            "current",
            "docs/new.txt",
            "file",
            2,
            2,
        )
        .await;
        drop(current);

        let settings = Arc::new(RwLock::new(Settings::default()));
        let service =
            SnapshotInspectionService::new(temp.path().join("config"), data_root, settings);
        (temp, service, current_path)
    }

    #[tokio::test]
    async fn prepared_session_serves_tree_nodes_after_the_filemap_is_removed() {
        let (_temp, service, current_path) = service_fixture().await;
        let summary = service
            .handle(&ControlRequest::new(
                "summary",
                "snapshot.inspect.summary",
                serde_json::json!({ "snapshotId": "current" }),
            ))
            .await;
        assert!(summary.ok);
        assert_eq!(
            summary.result.unwrap()["changes"]["added"].as_u64(),
            Some(1)
        );

        std::fs::remove_file(current_path).unwrap();
        let files = service
            .handle(&ControlRequest::new(
                "files",
                "snapshot.inspect.files",
                serde_json::json!({
                    "snapshotId": "current",
                    "presentation": "tree",
                    "scope": "changes",
                    "limit": 200,
                }),
            ))
            .await;
        assert!(files.ok);
        assert_eq!(
            files.result.unwrap()["entries"][0]["path"].as_str(),
            Some("docs")
        );
    }

    #[tokio::test]
    async fn control_socket_reuses_prepared_tree_after_the_filemap_is_removed() {
        let (temp, service, current_path) = service_fixture().await;
        let config_root = temp.path().join("config");
        std::fs::create_dir_all(&config_root).unwrap();
        let settings = Arc::new(RwLock::new(Settings::default()));
        let socket_path = temp.path().join("ipc").join("control.sock");
        let _server = crate::control_ipc::spawn_control_ipc_server(
            socket_path.clone(),
            crate::control_ipc::ControlContext {
                config_root: config_root.clone(),
                settings: settings.clone(),
                status_state: Arc::new(std::sync::Mutex::new(
                    crate::StatusRuntimeState::from_settings(&Settings::default()),
                )),
                backup_queue: Arc::new(std::sync::Mutex::new(crate::BackupQueue::default())),
                backup_queue_notify: Arc::new(tokio::sync::Notify::new()),
                settings_reload_requested: Arc::new(std::sync::atomic::AtomicBool::new(false)),
                lifecycle: Arc::new(crate::DaemonLifecycle::default()),
                runtime_logging: Arc::new(RwLock::new(
                    televy_backup_core::local_settings::resolve(&config_root),
                )),
                data_root: temp.path().join("data"),
                snapshot_inspection: Arc::new(service),
            },
        )
        .unwrap();

        let summary = request_over_control_socket(
            &socket_path,
            ControlRequest::new(
                "summary",
                "snapshot.inspect.summary",
                serde_json::json!({ "snapshotId": "current" }),
            ),
        )
        .await;
        assert!(summary.ok);

        std::fs::remove_file(current_path).unwrap();
        let files = request_over_control_socket(
            &socket_path,
            ControlRequest::new(
                "files",
                "snapshot.inspect.files",
                serde_json::json!({
                    "snapshotId": "current",
                    "presentation": "tree",
                    "scope": "changes",
                    "limit": 200,
                }),
            ),
        )
        .await;
        assert!(files.ok);
        assert_eq!(
            files.result.unwrap()["entries"][0]["path"].as_str(),
            Some("docs")
        );
    }
}
