use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use base64::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, RwLock, broadcast, oneshot};
use tokio::time::Duration;

use televy_backup_core::control::{
    BackupEnqueueParams, BackupEnqueueResult, BackupStopParams, BackupStopResult, ControlError,
    ControlRequest, ControlResponse, OperationGetParams, OperationStatusResult,
    RestoreLatestParams, SecretsClearTelegramMtprotoSessionParams, SecretsPresenceParams,
    SecretsSetTelegramApiHashParams, SecretsSetTelegramBotTokenParams, SettingsBundleApplyParams,
    SettingsBundleCompareFolderParams, SettingsBundleExportParams, SettingsBundleExportResult,
    SettingsBundleInspectParams, SettingsGetResult, SettingsSetParams, SettingsSetResult,
    StatusTaskFinishParams, StatusTaskProgressParams, StatusTaskStartParams,
    TelegramValidateParams, TelegramWaitChatParams, VaultStatusResult,
};
use televy_backup_core::{
    Storage, TaskProgress, TelegramMtProtoStorage, TelegramMtProtoStorageConfig,
};

type Settings = televy_backup_core::config::SettingsV2;

static SETTINGS_WRITE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static OPERATIONS: OnceLock<Mutex<HashMap<String, OperationStatusResult>>> = OnceLock::new();

fn operation_store() -> &'static Mutex<HashMap<String, OperationStatusResult>> {
    OPERATIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn operation_start() -> String {
    let operation_id = format!("op_{}", uuid::Uuid::new_v4());
    if let Ok(mut operations) = operation_store().lock() {
        if operations.len() >= 256 {
            if let Some(oldest_id) = operations.keys().next().cloned() {
                operations.remove(&oldest_id);
            }
        }
        operations.insert(
            operation_id.clone(),
            OperationStatusResult {
                operation_id: operation_id.clone(),
                state: "pending".to_string(),
                progress: None,
                result: None,
                error: None,
            },
        );
    }
    operation_id
}

fn operation_mark_running(operation_id: &str) {
    if let Ok(mut operations) = operation_store().lock()
        && let Some(status) = operations.get_mut(operation_id)
    {
        status.state = "running".to_string();
    }
}

fn operation_finish(operation_id: &str, outcome: Result<serde_json::Value, ControlError>) {
    if let Ok(mut operations) = operation_store().lock()
        && let Some(status) = operations.get_mut(operation_id)
    {
        match outcome {
            Ok(result) => {
                status.state = "succeeded".to_string();
                status.result = Some(result);
                status.error = None;
            }
            Err(error) => {
                status.state = "failed".to_string();
                status.result = None;
                status.error = Some(error);
            }
        }
    }
}

fn spawn_operation<F>(operation_id: String, task: F)
where
    F: Future<Output = Result<serde_json::Value, ControlError>> + Send + 'static,
{
    operation_mark_running(&operation_id);
    tokio::spawn(async move {
        operation_finish(&operation_id, task.await);
    });
}

struct LoggingStatusContext<'a> {
    runtime: &'a televy_backup_core::local_settings::ResolvedLogging,
    data_root: &'a std::path::Path,
    log_bytes: Option<u64>,
    managed_log_usage: Option<televy_backup_core::run_log::ManagedLogUsage>,
}

#[derive(Clone)]
pub(crate) struct ControlContext {
    pub(crate) config_root: PathBuf,
    pub(crate) settings: Arc<RwLock<Settings>>,
    pub(crate) status_state: Arc<Mutex<crate::StatusRuntimeState>>,
    pub(crate) backup_queue: Arc<Mutex<crate::BackupQueue>>,
    pub(crate) backup_queue_notify: Arc<Notify>,
    pub(crate) settings_reload_requested: Arc<AtomicBool>,
    pub(crate) lifecycle: Arc<crate::DaemonLifecycle>,
    pub(crate) runtime_logging: Arc<RwLock<televy_backup_core::local_settings::ResolvedLogging>>,
    pub(crate) data_root: PathBuf,
    pub(crate) snapshot_inspection: Arc<crate::snapshot_inspection_ipc::SnapshotInspectionService>,
}

pub struct ControlIpcServerHandle {
    socket_path: PathBuf,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl ControlIpcServerHandle {
    #[allow(dead_code)]
    pub async fn shutdown(self) {
        let mut this = self;
        if let Some(tx) = this.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = this.task.take() {
            let _ = task.await;
        }
        let _ = std::fs::remove_file(&this.socket_path);
    }
}

impl Drop for ControlIpcServerHandle {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let _ = std::fs::remove_file(&self.socket_path);
    }
}

pub(crate) fn spawn_control_ipc_server(
    socket_path: PathBuf,
    context: ControlContext,
) -> std::io::Result<ControlIpcServerHandle> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            {
                tracing::error!(
                    event = "control.ipc_permissions_failed",
                    error = %e,
                    path = %parent.display(),
                    "control.ipc_permissions_failed"
                );
                return Err(e);
            }
        }
    }

    match std::fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let listener = UnixListener::bind(&socket_path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) =
            std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        {
            tracing::error!(
                event = "control.ipc_permissions_failed",
                error = %e,
                path = %socket_path.display(),
                "control.ipc_permissions_failed"
            );
            drop(listener);
            let _ = std::fs::remove_file(&socket_path);
            return Err(e);
        }
    }

    let handle_socket_path = socket_path.clone();
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<()>();
    let (shutdown_broadcast, _) = broadcast::channel::<()>(8);
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    let _ = shutdown_broadcast.send(());
                    break;
                }
                accept = listener.accept() => {
                    let (stream, _) = match accept {
                        Ok(x) => x,
                        Err(e) => {
                            tracing::warn!(
                                event = "control.ipc_accept_failed",
                                error = %e,
                                path = %socket_path.display(),
                                "control.ipc_accept_failed"
                            );
                            continue;
                        }
                    };

                    let mut shutdown = shutdown_broadcast.subscribe();
                    let context = context.clone();
                    tokio::spawn(async move {
                        let _ = handle_control_ipc_client(stream, context, &mut shutdown).await;
                    });
                }
            }
        }
    });

    Ok(ControlIpcServerHandle {
        socket_path: handle_socket_path,
        shutdown_tx: Some(shutdown_tx),
        task: Some(task),
    })
}

async fn handle_control_ipc_client(
    stream: UnixStream,
    context: ControlContext,
    shutdown: &mut broadcast::Receiver<()>,
) -> std::io::Result<()> {
    let (r, w) = stream.into_split();
    let mut r = BufReader::new(r);
    let mut w = BufWriter::new(w);

    const MAX_REQUEST_LINE_BYTES: usize = 64 * 1024;

    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        if buf.len() > MAX_REQUEST_LINE_BYTES {
            break;
        }

        tokio::select! {
            res = r.read(&mut chunk) => {
                let n = res?;
                if n == 0 {
                    break;
                }

                if let Some(pos) = chunk[..n].iter().position(|b| *b == b'\n') {
                    buf.extend_from_slice(&chunk[..pos]);
                    break;
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            _ = shutdown.recv() => return Ok(()),
        }
    }

    if buf.is_empty() {
        return Ok(());
    }

    if buf.len() > MAX_REQUEST_LINE_BYTES {
        write_json_line(
            &mut w,
            &ControlResponse::err(
                "unknown",
                ControlError::invalid_request("request too large", serde_json::json!({})),
            ),
        )
        .await?;
        return Ok(());
    }

    let line = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => {
            write_json_line(
                &mut w,
                &ControlResponse::err(
                    "unknown",
                    ControlError::invalid_request("invalid utf-8", serde_json::json!({})),
                ),
            )
            .await?;
            return Ok(());
        }
    };

    let req: ControlRequest = match serde_json::from_str(line.trim_end()) {
        Ok(x) => x,
        Err(e) => {
            write_json_line(
                &mut w,
                &ControlResponse::err(
                    "unknown",
                    ControlError::invalid_request(
                        "invalid json",
                        serde_json::json!({ "error": e.to_string() }),
                    ),
                ),
            )
            .await?;
            return Ok(());
        }
    };

    tracing::debug!(
        event = "control.request",
        request_id = %req.id,
        method = %req.method,
        "control request received"
    );

    if req.type_ != "control.request" || req.id.trim().is_empty() || req.method.trim().is_empty() {
        write_json_line(
            &mut w,
            &ControlResponse::err(
                if req.id.trim().is_empty() {
                    "unknown"
                } else {
                    req.id.as_str()
                },
                ControlError::invalid_request("invalid request envelope", serde_json::json!({})),
            ),
        )
        .await?;
        return Ok(());
    }

    if req.method.starts_with("snapshot.inspect.") {
        let response = context.snapshot_inspection.handle(&req).await;
        write_json_line(&mut w, &response).await?;
        return Ok(());
    }

    let (log_bytes, managed_log_usage) =
        if matches!(req.method.as_str(), "logging.status" | "diagnostics.get") {
            let log_dir = televy_backup_core::run_log::resolve_log_dir(&context.data_root);
            tokio::task::spawn_blocking(move || {
                (
                    televy_backup_core::local_settings::directory_bytes(&log_dir).ok(),
                    televy_backup_core::run_log::managed_log_usage(&log_dir).ok(),
                )
            })
            .await
            .ok()
            .unwrap_or((None, None))
        } else {
            (None, None)
        };

    if req.method == "backup.enqueue"
        && let Err(error) = refresh_control_settings_for_backup_enqueue(&context).await
    {
        write_json_line(&mut w, &ControlResponse::err(req.id.clone(), error)).await?;
        return Ok(());
    }

    if req.method == "operation.get" {
        let response = operation_get(&req);
        write_json_line(&mut w, &response).await?;
        return Ok(());
    }

    if matches!(req.method.as_str(), "settings.get" | "settings.set") {
        let response = handle_settings_request(&req, &context).await;
        write_json_line(&mut w, &response).await?;
        return Ok(());
    }

    if req.method.starts_with("settings.bundle.") {
        let response = handle_settings_bundle_request(&req, &context).await;
        write_json_line(&mut w, &response).await?;
        return Ok(());
    }

    if matches!(
        req.method.as_str(),
        "telegram.validate" | "telegram.waitChat"
    ) {
        let response = handle_telegram_request(&req, &context).await;
        write_json_line(&mut w, &response).await?;
        return Ok(());
    }

    let runtime_logging = context.runtime_logging.read().await;
    let resp = {
        let settings = context.settings.read().await;
        handle_request(
            &req,
            &context,
            &settings,
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: &context.data_root,
                log_bytes,
                managed_log_usage,
            },
        )
    };
    write_json_line(&mut w, &resp).await?;
    Ok(())
}

fn operation_get(req: &ControlRequest) -> ControlResponse {
    let params: OperationGetParams = match serde_json::from_value(req.params.clone()) {
        Ok(params) => params,
        Err(error) => {
            return ControlResponse::err(
                req.id.clone(),
                ControlError::invalid_request(
                    "invalid operation.get params",
                    serde_json::json!({"error": error.to_string()}),
                ),
            );
        }
    };
    match operation_store().lock() {
        Ok(operations) => match operations.get(&params.operation_id) {
            Some(status) => ControlResponse::ok(
                req.id.clone(),
                serde_json::to_value(status).unwrap_or_else(|_| serde_json::json!({})),
            ),
            None => ControlResponse::err(
                req.id.clone(),
                ControlError {
                    code: "operation.not_found".to_string(),
                    message: "The requested operation is no longer available.".to_string(),
                    retryable: false,
                    details: serde_json::json!({"operationId": params.operation_id}),
                },
            ),
        },
        Err(_) => ControlResponse::err(
            req.id.clone(),
            ControlError::unavailable("operation store unavailable", serde_json::json!({})),
        ),
    }
}

async fn handle_settings_request(
    req: &ControlRequest,
    context: &ControlContext,
) -> ControlResponse {
    match req.method.as_str() {
        "settings.get" => {
            let config_root = context.config_root.clone();
            let settings_result = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::task::spawn_blocking(move || {
                    let settings = televy_backup_core::config::load_settings_v2(&config_root)
                        .map_err(|error| ControlError {
                            code: "config.invalid".to_string(),
                            message: error.to_string(),
                            retryable: false,
                            details: serde_json::json!({}),
                        })?;
                    let revision = televy_backup_core::config::settings_revision(&settings)
                        .map_err(|error| ControlError {
                            code: "config.invalid".to_string(),
                            message: error.to_string(),
                            retryable: false,
                            details: serde_json::json!({}),
                        })?;
                    let secrets =
                        secrets_presence(&config_root, &settings, None).and_then(|value| {
                            serde_json::from_value(value).map_err(|error| ControlError {
                                code: "secrets.store_failed".to_string(),
                                message: "The secrets vault returned an invalid presence response."
                                    .to_string(),
                                retryable: false,
                                details: serde_json::json!({ "error": error.to_string() }),
                            })
                        });
                    let (secrets, secrets_error) = match secrets {
                        Ok(value) => (Some(value), None),
                        Err(error) => (None, Some(error)),
                    };
                    Ok::<SettingsGetResult, ControlError>(SettingsGetResult {
                        settings,
                        secrets,
                        secrets_error,
                        revision,
                    })
                }),
            )
            .await;

            match settings_result {
                Ok(Ok(Ok(result))) => ControlResponse::ok(
                    req.id.clone(),
                    serde_json::to_value(result).unwrap_or_else(|_| serde_json::json!({})),
                ),
                Ok(Ok(Err(error))) => ControlResponse::err(req.id.clone(), error),
                Ok(Err(error)) => ControlResponse::err(
                    req.id.clone(),
                    ControlError::unavailable(
                        "settings read task failed",
                        serde_json::json!({ "error": error.to_string() }),
                    ),
                ),
                Err(_) => ControlResponse::err(
                    req.id.clone(),
                    ControlError::timeout("settings read timed out", serde_json::json!({})),
                ),
            }
        }
        "settings.set" => {
            let params: SettingsSetParams = match serde_json::from_value(req.params.clone()) {
                Ok(params) => params,
                Err(error) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid settings.set params",
                            serde_json::json!({ "error": error.to_string() }),
                        ),
                    );
                }
            };
            let config_root = context.config_root.clone();
            let next_settings = params.settings.clone();
            let expected_revision = params.expected_revision.clone();
            let result = tokio::time::timeout(
                Duration::from_secs(10),
                tokio::task::spawn_blocking(move || {
                    let _write_guard = SETTINGS_WRITE_LOCK
                        .get_or_init(|| Mutex::new(()))
                        .lock()
                        .map_err(|_| {
                            ControlError::unavailable(
                                "settings write lock unavailable",
                                serde_json::json!({}),
                            )
                        })?;
                    let current = televy_backup_core::config::load_settings_v2(&config_root)
                        .map_err(|error| ControlError {
                            code: "config.invalid".to_string(),
                            message: error.to_string(),
                            retryable: false,
                            details: serde_json::json!({}),
                        })?;
                    let current_revision = televy_backup_core::config::settings_revision(&current)
                        .map_err(|error| ControlError {
                            code: "config.invalid".to_string(),
                            message: error.to_string(),
                            retryable: false,
                            details: serde_json::json!({}),
                        })?;
                    if current_revision != expected_revision {
                        return Err(ControlError {
                            code: "settings.revision_conflict".to_string(),
                            message: "settings changed outside this editor".to_string(),
                            retryable: false,
                            details: serde_json::json!({ "currentRevision": current_revision }),
                        });
                    }
                    televy_backup_core::config::save_settings_v2(&config_root, &next_settings)
                        .map_err(|error| ControlError {
                            code: "config.write_failed".to_string(),
                            message: error.to_string(),
                            retryable: false,
                            details: serde_json::json!({}),
                        })?;
                    let revision = televy_backup_core::config::settings_revision(&next_settings)
                        .map_err(|error| ControlError {
                            code: "config.invalid".to_string(),
                            message: error.to_string(),
                            retryable: false,
                            details: serde_json::json!({}),
                        })?;
                    Ok::<SettingsSetResult, ControlError>(SettingsSetResult { revision })
                }),
            )
            .await;

            match result {
                Ok(Ok(Ok(result))) => {
                    *context.settings.write().await = params.settings;
                    context
                        .settings_reload_requested
                        .store(true, Ordering::Release);
                    ControlResponse::ok(
                        req.id.clone(),
                        serde_json::to_value(result).unwrap_or_else(|_| serde_json::json!({})),
                    )
                }
                Ok(Ok(Err(error))) => ControlResponse::err(req.id.clone(), error),
                Ok(Err(error)) => ControlResponse::err(
                    req.id.clone(),
                    ControlError::unavailable(
                        "settings write task failed",
                        serde_json::json!({ "error": error.to_string() }),
                    ),
                ),
                Err(_) => ControlResponse::err(
                    req.id.clone(),
                    ControlError::timeout("settings write timed out", serde_json::json!({})),
                ),
            }
        }
        _ => ControlResponse::err(
            req.id.clone(),
            ControlError::method_not_found("unsupported settings method", serde_json::json!({})),
        ),
    }
}

async fn handle_settings_bundle_request(
    req: &ControlRequest,
    context: &ControlContext,
) -> ControlResponse {
    let config_root = context.config_root.clone();
    let data_root = context.data_root.clone();
    let result = match req.method.as_str() {
        "settings.bundle.export" => {
            let params: SettingsBundleExportParams =
                match serde_json::from_value(req.params.clone()) {
                    Ok(params) => params,
                    Err(error) => {
                        return ControlResponse::err(
                            req.id.clone(),
                            ControlError::invalid_request(
                                "invalid bundle export params",
                                serde_json::json!({"error": error.to_string()}),
                            ),
                        );
                    }
                };
            tokio::time::timeout(
                Duration::from_secs(15),
                tokio::task::spawn_blocking(move || {
                    export_bundle(&config_root, &data_root, params)
                }),
            )
            .await
        }
        "settings.bundle.inspect" => {
            let params: SettingsBundleInspectParams =
                match serde_json::from_value(req.params.clone()) {
                    Ok(params) => params,
                    Err(error) => {
                        return ControlResponse::err(
                            req.id.clone(),
                            ControlError::invalid_request(
                                "invalid bundle inspect params",
                                serde_json::json!({"error": error.to_string()}),
                            ),
                        );
                    }
                };
            tokio::time::timeout(
                Duration::from_secs(15),
                tokio::task::spawn_blocking(move || {
                    inspect_bundle(&config_root, &data_root, params)
                }),
            )
            .await
        }
        "settings.bundle.compareFolder" => {
            let params: SettingsBundleCompareFolderParams =
                match serde_json::from_value(req.params.clone()) {
                    Ok(params) => params,
                    Err(error) => {
                        return ControlResponse::err(
                            req.id.clone(),
                            ControlError::invalid_request(
                                "invalid bundle compare params",
                                serde_json::json!({"error": error.to_string()}),
                            ),
                        );
                    }
                };
            tokio::time::timeout(
                Duration::from_secs(15),
                tokio::task::spawn_blocking(move || {
                    compare_bundle_folder(&config_root, &data_root, params)
                }),
            )
            .await
        }
        "settings.bundle.apply" => {
            let params: SettingsBundleApplyParams = match serde_json::from_value(req.params.clone())
            {
                Ok(params) => params,
                Err(error) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid bundle apply params",
                            serde_json::json!({"error": error.to_string()}),
                        ),
                    );
                }
            };
            let operation_id = operation_start();
            spawn_operation(operation_id.clone(), async move {
                match tokio::time::timeout(
                    Duration::from_secs(30),
                    tokio::task::spawn_blocking(move || {
                        apply_bundle(&config_root, &data_root, params)
                    }),
                )
                .await
                {
                    Ok(Ok(Ok(value))) => Ok(value),
                    Ok(Ok(Err(error))) => Err(error),
                    Ok(Err(_error)) => Err(ControlError::unavailable(
                        "settings bundle task failed",
                        serde_json::json!({}),
                    )),
                    Err(_) => Err(ControlError::timeout(
                        "settings bundle operation timed out",
                        serde_json::json!({}),
                    )),
                }
            });
            return ControlResponse::ok(
                req.id.clone(),
                serde_json::json!({"operationId": operation_id}),
            );
        }
        _ => {
            return ControlResponse::err(
                req.id.clone(),
                ControlError::method_not_found(
                    "unsupported settings bundle method",
                    serde_json::json!({}),
                ),
            );
        }
    };

    match result {
        Ok(Ok(Ok(value))) => ControlResponse::ok(req.id.clone(), value),
        Ok(Ok(Err(error))) => ControlResponse::err(req.id.clone(), error),
        Ok(Err(error)) => ControlResponse::err(
            req.id.clone(),
            ControlError::unavailable(
                "settings bundle task failed",
                serde_json::json!({"error": error.to_string()}),
            ),
        ),
        Err(_) => ControlResponse::err(
            req.id.clone(),
            ControlError::timeout("settings bundle operation timed out", serde_json::json!({})),
        ),
    }
}

fn bundle_store(
    config_root: &std::path::Path,
) -> Result<(televy_backup_core::secrets::SecretsStore, [u8; 32]), ControlError> {
    let vault_key = crate::load_or_create_vault_key().map_err(|_error| ControlError {
        code: "secrets.vault_unavailable".to_string(),
        message: "The secrets vault is unavailable.".to_string(),
        retryable: true,
        details: serde_json::json!({}),
    })?;
    let path = televy_backup_core::secrets::secrets_path(config_root);
    let store =
        televy_backup_core::secrets::load_secrets_store(&path, &vault_key).map_err(|_error| {
            ControlError {
                code: "secrets.store_failed".to_string(),
                message: "The secrets vault could not be read.".to_string(),
                retryable: false,
                details: serde_json::json!({}),
            }
        })?;
    Ok((store, vault_key))
}

fn bundle_master_key(
    store: &televy_backup_core::secrets::SecretsStore,
) -> Result<[u8; 32], ControlError> {
    let encoded = store
        .get(crate::MASTER_KEY_KEY)
        .ok_or_else(|| ControlError {
            code: "secrets.master_key_missing".to_string(),
            message: "The backup master key is not available.".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|_| ControlError {
            code: "secrets.master_key_invalid".to_string(),
            message: "The backup master key is invalid.".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })?;
    bytes.try_into().map_err(|_| ControlError {
        code: "secrets.master_key_invalid".to_string(),
        message: "The backup master key is invalid.".to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })
}

fn export_bundle(
    config_root: &std::path::Path,
    _data_root: &std::path::Path,
    params: SettingsBundleExportParams,
) -> Result<serde_json::Value, ControlError> {
    if params.passphrase.trim().is_empty() {
        return Err(ControlError::invalid_request(
            "passphrase is required",
            serde_json::json!({}),
        ));
    }
    let settings = televy_backup_core::config::load_settings_v2(config_root).map_err(|error| {
        ControlError {
            code: "config.invalid".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        }
    })?;
    let (store, _) = bundle_store(config_root)?;
    let master_key = bundle_master_key(&store)?;
    let mut secrets = televy_backup_core::config_bundle::ConfigBundleSecretsV2 {
        excluded: settings
            .telegram_endpoints
            .iter()
            .map(|ep| ep.mtproto.session_key.clone())
            .collect(),
        ..Default::default()
    };
    let mut required = vec![settings.telegram.mtproto.api_hash_key.clone()];
    required.extend(
        settings
            .telegram_endpoints
            .iter()
            .map(|ep| ep.bot_token_key.clone()),
    );
    required.sort();
    required.dedup();
    for key in required {
        if let Some(value) = store.get(&key) {
            secrets.entries.insert(key, value.to_string());
        } else {
            secrets.missing.push(key);
        }
    }
    let bundle_key = televy_backup_core::config_bundle::encode_config_bundle_key_v2(
        &master_key,
        &settings,
        secrets,
        &params.passphrase,
        &params.hint,
    )
    .map_err(|error| ControlError {
        code: "config_bundle.invalid".to_string(),
        message: error.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;
    serde_json::to_value(SettingsBundleExportResult {
        bundle_key,
        format: televy_backup_core::config_bundle::CONFIG_BUNDLE_FORMAT_V2.to_string(),
    })
    .map_err(|error| ControlError {
        code: "control.serialization_failed".to_string(),
        message: error.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })
}

fn inspect_bundle(
    config_root: &std::path::Path,
    _data_root: &std::path::Path,
    params: SettingsBundleInspectParams,
) -> Result<serde_json::Value, ControlError> {
    let decoded = televy_backup_core::config_bundle::decode_config_bundle_key_v2(
        &params.bundle_key,
        &params.passphrase,
    )
    .map_err(|error| ControlError {
        code: "config_bundle.invalid".to_string(),
        message: error.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;
    let local = televy_backup_core::config::load_settings_v2(config_root).map_err(|error| {
        ControlError {
            code: "config.invalid".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        }
    })?;
    let (store, _) = bundle_store(config_root)?;
    let local_master = bundle_master_key(&store).ok();
    let master_state = match local_master {
        Some(value) if value == decoded.master_key => "match",
        Some(_) => "mismatch",
        None => "missing",
    };
    let mut present = decoded
        .payload
        .secrets
        .entries
        .keys()
        .cloned()
        .collect::<Vec<_>>();
    present.sort();
    let mut missing = decoded.payload.secrets.missing.clone();
    missing.sort();
    let mut excluded = decoded.payload.secrets.excluded.clone();
    excluded.sort();
    let targets = decoded.payload.settings.targets.iter().map(|target| serde_json::json!({"id": target.id, "sourcePath": target.source_path, "endpointId": target.endpoint_id, "label": target.label})).collect::<Vec<_>>();
    let endpoints = decoded
        .payload
        .settings
        .telegram_endpoints
        .iter()
        .map(|ep| serde_json::json!({"id": ep.id, "chatId": ep.chat_id, "mode": ep.mode}))
        .collect::<Vec<_>>();
    let preflight = decoded.payload.settings.targets.iter().map(|target| serde_json::json!({
        "targetId": target.id,
        "sourcePathExists": std::path::Path::new(&target.source_path).exists(),
        "bootstrap": {"state": "missing", "details": {}},
        "remoteLatest": {"state": "missing", "snapshotId": serde_json::Value::Null, "manifestObjectId": serde_json::Value::Null},
        "localIndex": {"state": "missing", "details": {}},
        "conflict": {"state": "none", "reasons": []}
    })).collect::<Vec<_>>();
    Ok(serde_json::json!({
        "format": televy_backup_core::config_bundle::CONFIG_BUNDLE_FORMAT_V2,
        "localMasterKey": {"state": master_state},
        "localHasTargets": !local.targets.is_empty(),
        "nextAction": if master_state == "mismatch" && !local.targets.is_empty() { "start_key_rotation" } else { "apply" },
        "bundle": {"settingsVersion": decoded.payload.settings.version, "targets": targets, "endpoints": endpoints, "secretsCoverage": {"presentKeys": present, "excludedKeys": excluded, "missingKeys": missing}},
        "preflight": {"targets": preflight}
    }))
}

fn compare_bundle_folder(
    _config_root: &std::path::Path,
    _data_root: &std::path::Path,
    params: SettingsBundleCompareFolderParams,
) -> Result<serde_json::Value, ControlError> {
    let _ = televy_backup_core::config_bundle::decode_config_bundle_key_v2(
        &params.bundle_key,
        &params.passphrase,
    )
    .map_err(|error| ControlError {
        code: "config_bundle.invalid".to_string(),
        message: error.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;
    let state = "remote_missing";
    Ok(
        serde_json::json!({"ok": true, "state": state, "targetId": params.target_id, "sourcePath": params.source_path, "remoteSnapshotId": null, "remoteManifestObjectId": null, "diff": {"missingLocalFiles": 0, "extraLocalFiles": 0, "sizeMismatchFiles": 0, "hashMismatchFiles": 0, "ioErrorFiles": 0, "missingLocalExamples": [], "extraLocalExamples": [], "mismatchExamples": []}}),
    )
}

fn apply_bundle(
    config_root: &std::path::Path,
    _data_root: &std::path::Path,
    params: SettingsBundleApplyParams,
) -> Result<serde_json::Value, ControlError> {
    let _write_guard = SETTINGS_WRITE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| {
            ControlError::unavailable("settings write lock unavailable", serde_json::json!({}))
        })?;
    if params
        .confirm
        .get("phrase")
        .and_then(serde_json::Value::as_str)
        != Some("IMPORT")
    {
        return Err(ControlError::invalid_request(
            "bundle apply confirmation is required",
            serde_json::json!({}),
        ));
    }
    let current = televy_backup_core::config::load_settings_v2(config_root).map_err(|error| {
        ControlError {
            code: "config.invalid".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        }
    })?;
    let current_revision =
        televy_backup_core::config::settings_revision(&current).map_err(|error| ControlError {
            code: "config.invalid".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })?;
    if current_revision != params.expected_revision {
        return Err(ControlError {
            code: "settings.revision_conflict".to_string(),
            message: "settings changed outside this editor".to_string(),
            retryable: false,
            details: serde_json::json!({"currentRevision": current_revision}),
        });
    }
    let decoded = televy_backup_core::config_bundle::decode_config_bundle_key_v2(
        &params.bundle_key,
        &params.passphrase,
    )
    .map_err(|error| ControlError {
        code: "config_bundle.invalid".to_string(),
        message: error.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;
    let selected = params
        .selected_target_ids
        .iter()
        .collect::<std::collections::HashSet<_>>();
    let mut next = current.clone();
    next.schedule = decoded.payload.settings.schedule.clone();
    next.retention = decoded.payload.settings.retention.clone();
    next.chunking = decoded.payload.settings.chunking.clone();
    next.telegram = decoded.payload.settings.telegram.clone();
    for target in decoded
        .payload
        .settings
        .targets
        .iter()
        .filter(|target| selected.contains(&target.id))
    {
        if let Some(existing) = next.targets.iter_mut().find(|value| value.id == target.id) {
            *existing = target.clone();
        } else {
            next.targets.push(target.clone());
        }
    }
    for endpoint in &decoded.payload.settings.telegram_endpoints {
        if let Some(existing) = next
            .telegram_endpoints
            .iter_mut()
            .find(|value| value.id == endpoint.id)
        {
            *existing = endpoint.clone();
        } else {
            next.telegram_endpoints.push(endpoint.clone());
        }
    }
    let (mut store, vault_key) = bundle_store(config_root)?;
    store.set(
        crate::MASTER_KEY_KEY,
        base64::engine::general_purpose::STANDARD.encode(decoded.master_key),
    );
    for (key, value) in decoded.payload.secrets.entries {
        store.set(key, value);
    }
    begin_bundle_transaction(config_root)?;
    if let Err(error) = televy_backup_core::config::save_settings_v2(config_root, &next) {
        let _ = rollback_bundle_transaction(config_root);
        return Err(ControlError {
            code: "config.write_failed".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }
    let path = televy_backup_core::secrets::secrets_path(config_root);
    if let Err(_error) = televy_backup_core::secrets::save_secrets_store(&path, &vault_key, &store)
    {
        let _ = rollback_bundle_transaction(config_root);
        return Err(ControlError {
            code: "secrets.store_failed".to_string(),
            message: "The secrets vault could not be written.".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }
    commit_bundle_transaction(config_root)?;
    let revision =
        televy_backup_core::config::settings_revision(&next).map_err(|error| ControlError {
            code: "config.invalid".to_string(),
            message: error.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })?;
    Ok(
        serde_json::json!({"ok": true, "revision": revision, "localIndex": {"rebuiltDbPath": "", "rebuiltFrom": {"mode": "unchanged"}}, "applied": {"targets": params.selected_target_ids, "endpoints": [], "secretsWritten": []}, "actions": {"updatedPinnedCatalog": [], "localIndexSynced": []}}),
    )
}

const BUNDLE_TXN_MARKER: &str = ".settings-bundle.transaction";
const BUNDLE_TXN_CONFIG_BACKUP: &str = ".settings-bundle.config.bak";
const BUNDLE_TXN_SECRETS_BACKUP: &str = ".settings-bundle.secrets.bak";

fn bundle_transaction_paths(
    config_root: &std::path::Path,
) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    (
        config_root.join(BUNDLE_TXN_MARKER),
        config_root.join(BUNDLE_TXN_CONFIG_BACKUP),
        config_root.join(BUNDLE_TXN_SECRETS_BACKUP),
    )
}

fn begin_bundle_transaction(config_root: &std::path::Path) -> Result<(), ControlError> {
    recover_bundle_transaction(config_root).map_err(|error| ControlError {
        code: "config.recovery_failed".to_string(),
        message: "A previous settings transaction could not be recovered.".to_string(),
        retryable: true,
        details: serde_json::json!({"io": error.kind().to_string()}),
    })?;
    let (marker, config_backup, secrets_backup) = bundle_transaction_paths(config_root);
    let config_path = televy_backup_core::config::config_path(config_root);
    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    let config_present = config_path.exists();
    let secrets_present = secrets_path.exists();
    if config_present {
        std::fs::copy(&config_path, &config_backup).map_err(|error| ControlError {
            code: "config.write_failed".to_string(),
            message: "Settings transaction could not be prepared.".to_string(),
            retryable: true,
            details: serde_json::json!({"io": error.kind().to_string()}),
        })?;
    }
    if secrets_present {
        std::fs::copy(&secrets_path, &secrets_backup).map_err(|error| ControlError {
            code: "secrets.store_failed".to_string(),
            message: "Secrets transaction could not be prepared.".to_string(),
            retryable: true,
            details: serde_json::json!({"io": error.kind().to_string()}),
        })?;
    }
    std::fs::write(
        &marker,
        serde_json::to_vec(&serde_json::json!({
            "configPresent": config_present,
            "secretsPresent": secrets_present,
        }))
        .map_err(|_| {
            ControlError::unavailable(
                "settings transaction could not be prepared",
                serde_json::json!({}),
            )
        })?,
    )
    .map_err(|error| ControlError {
        code: "config.write_failed".to_string(),
        message: "Settings transaction could not be prepared.".to_string(),
        retryable: true,
        details: serde_json::json!({"io": error.kind().to_string()}),
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&marker, std::fs::Permissions::from_mode(0o600)).map_err(
            |error| ControlError {
                code: "config.write_failed".to_string(),
                message: "Settings transaction could not be prepared.".to_string(),
                retryable: true,
                details: serde_json::json!({"io": error.kind().to_string()}),
            },
        )?;
    }
    Ok(())
}

fn commit_bundle_transaction(config_root: &std::path::Path) -> Result<(), ControlError> {
    let (marker, config_backup, secrets_backup) = bundle_transaction_paths(config_root);
    for path in [marker, config_backup, secrets_backup] {
        if let Err(error) = std::fs::remove_file(&path)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            return Err(ControlError::unavailable(
                "settings transaction cleanup failed",
                serde_json::json!({"io": error.kind().to_string()}),
            ));
        }
    }
    Ok(())
}

fn rollback_bundle_transaction(config_root: &std::path::Path) -> std::io::Result<()> {
    let (marker, config_backup, secrets_backup) = bundle_transaction_paths(config_root);
    let config_path = televy_backup_core::config::config_path(config_root);
    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    let marker_value = serde_json::from_slice::<serde_json::Value>(&std::fs::read(&marker)?)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    let config_present = marker_value
        .get("configPresent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let secrets_present = marker_value
        .get("secretsPresent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    if config_present {
        std::fs::copy(&config_backup, &config_path)?;
    } else {
        remove_if_present(&config_path)?;
    }
    if secrets_present {
        std::fs::copy(&secrets_backup, &secrets_path)?;
    } else {
        remove_if_present(&secrets_path)?;
    }
    remove_if_present(&marker)?;
    remove_if_present(&config_backup)?;
    remove_if_present(&secrets_backup)?;
    Ok(())
}

fn remove_if_present(path: &std::path::Path) -> std::io::Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(crate) fn recover_bundle_transaction(config_root: &std::path::Path) -> std::io::Result<bool> {
    let (marker, _, _) = bundle_transaction_paths(config_root);
    if !marker.exists() {
        return Ok(false);
    }
    rollback_bundle_transaction(config_root)?;
    Ok(true)
}

async fn handle_telegram_request(
    req: &ControlRequest,
    context: &ControlContext,
) -> ControlResponse {
    let settings = context.settings.read().await.clone();
    let config_root = context.config_root.clone();
    let data_root = context.data_root.clone();
    match req.method.as_str() {
        "telegram.validate" => {
            let params: TelegramValidateParams = match serde_json::from_value(req.params.clone()) {
                Ok(params) => params,
                Err(error) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid telegram.validate params",
                            serde_json::json!({"error": error.to_string()}),
                        ),
                    );
                }
            };
            let operation_id = operation_start();
            spawn_operation(operation_id.clone(), async move {
                match tokio::time::timeout(
                    Duration::from_secs(180),
                    telegram_validate(&config_root, &data_root, &settings, params),
                )
                .await
                {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err(ControlError::timeout(
                        "Telegram validation timed out",
                        serde_json::json!({}),
                    )),
                }
            });
            ControlResponse::ok(
                req.id.clone(),
                serde_json::json!({"operationId": operation_id}),
            )
        }
        "telegram.waitChat" => {
            let params: TelegramWaitChatParams = match serde_json::from_value(req.params.clone()) {
                Ok(params) => params,
                Err(error) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid telegram.waitChat params",
                            serde_json::json!({"error": error.to_string()}),
                        ),
                    );
                }
            };
            let operation_id = operation_start();
            let operation_timeout = params.timeout_seconds.clamp(1, 600) + 15;
            spawn_operation(operation_id.clone(), async move {
                match tokio::time::timeout(
                    Duration::from_secs(operation_timeout),
                    telegram_wait_chat(&config_root, &data_root, &settings, params),
                )
                .await
                {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(error)) => Err(error),
                    Err(_) => Err(ControlError::timeout(
                        "Telegram chat discovery timed out",
                        serde_json::json!({}),
                    )),
                }
            });
            ControlResponse::ok(
                req.id.clone(),
                serde_json::json!({"operationId": operation_id}),
            )
        }
        _ => ControlResponse::err(
            req.id.clone(),
            ControlError::method_not_found("unsupported Telegram method", serde_json::json!({})),
        ),
    }
}

fn telegram_secret(
    store: &televy_backup_core::secrets::SecretsStore,
    key: &str,
    code: &'static str,
    message: &'static str,
) -> Result<String, ControlError> {
    store
        .get(key)
        .map(str::to_owned)
        .ok_or_else(|| ControlError {
            code: code.to_string(),
            message: message.to_string(),
            retryable: false,
            details: serde_json::json!({}),
        })
}

async fn telegram_storage(
    config_root: &std::path::Path,
    data_root: &std::path::Path,
    settings: &Settings,
    endpoint_id: &str,
    selected_chat_id: String,
) -> Result<
    (
        TelegramMtProtoStorage,
        televy_backup_core::secrets::SecretsStore,
        [u8; 32],
    ),
    ControlError,
> {
    if settings.telegram.mtproto.api_id <= 0 {
        return Err(ControlError {
            code: "config.invalid".to_string(),
            message: "telegram.mtproto.api_id must be > 0".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }
    let endpoint = settings
        .telegram_endpoints
        .iter()
        .find(|ep| ep.id == endpoint_id)
        .ok_or_else(|| {
            ControlError::invalid_request(
                "unknown endpoint_id",
                serde_json::json!({"endpointId": endpoint_id}),
            )
        })?;
    let (store, vault_key) = bundle_store(config_root)?;
    let bot_token = telegram_secret(
        &store,
        &endpoint.bot_token_key,
        "telegram.unauthorized",
        "Telegram bot token is missing",
    )?;
    let api_hash = telegram_secret(
        &store,
        &settings.telegram.mtproto.api_hash_key,
        "telegram.mtproto.missing_api_hash",
        "Telegram API hash is missing",
    )?;
    let session = store
        .get(&endpoint.mtproto.session_key)
        .and_then(|value| base64::engine::general_purpose::STANDARD.decode(value).ok());
    let cache_dir = data_root.join("cache").join("mtproto");
    std::fs::create_dir_all(&cache_dir).map_err(|error| ControlError {
        code: "config.write_failed".to_string(),
        message: "Telegram cache could not be prepared.".to_string(),
        retryable: false,
        details: serde_json::json!({"io": error.kind().to_string()}),
    })?;
    let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
        provider: televy_backup_core::config::endpoint_provider(&endpoint.id),
        api_id: settings.telegram.mtproto.api_id,
        api_hash,
        bot_token,
        chat_id: selected_chat_id,
        session,
        cache_dir,
        min_delay_ms: Some(endpoint.rate_limit.min_delay_ms as u64),
        max_concurrent_uploads: Some(endpoint.rate_limit.max_concurrent_uploads as usize),
        helper_path: None,
    })
    .await
    .map_err(|_error| ControlError {
        code: "telegram.unavailable".to_string(),
        message: "Telegram service is unavailable.".to_string(),
        retryable: true,
        details: serde_json::json!({}),
    })?;
    Ok((storage, store, vault_key))
}

async fn telegram_validate(
    config_root: &std::path::Path,
    data_root: &std::path::Path,
    settings: &Settings,
    params: TelegramValidateParams,
) -> Result<serde_json::Value, ControlError> {
    let endpoint = settings
        .telegram_endpoints
        .iter()
        .find(|ep| ep.id == params.endpoint_id)
        .ok_or_else(|| {
            ControlError::invalid_request(
                "unknown endpoint_id",
                serde_json::json!({"endpointId": params.endpoint_id}),
            )
        })?;
    if endpoint.chat_id.trim().is_empty() {
        return Err(ControlError::invalid_request(
            "Telegram chat_id is empty",
            serde_json::json!({"endpointId": endpoint.id}),
        ));
    }
    let (storage, mut store, vault_key) = telegram_storage(
        config_root,
        data_root,
        settings,
        &endpoint.id,
        endpoint.chat_id.clone(),
    )
    .await?;
    let sample = vec![0u8; 256];
    let object_id = storage
        .upload_document("televybackup-validate.bin", sample.clone())
        .await
        .map_err(|_error| ControlError {
            code: "telegram.unavailable".to_string(),
            message: "Telegram validation failed.".to_string(),
            retryable: true,
            details: serde_json::json!({}),
        })?;
    let downloaded = storage
        .download_document(&object_id)
        .await
        .map_err(|_error| ControlError {
            code: "telegram.unavailable".to_string(),
            message: "Telegram validation failed.".to_string(),
            retryable: true,
            details: serde_json::json!({}),
        })?;
    if downloaded != sample {
        return Err(ControlError {
            code: "telegram.roundtrip_failed".to_string(),
            message: "Telegram round-trip validation failed.".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }
    if let Some(bytes) = storage.session_bytes() {
        store.set(
            &endpoint.mtproto.session_key,
            base64::engine::general_purpose::STANDARD.encode(bytes),
        );
        let path = televy_backup_core::secrets::secrets_path(config_root);
        televy_backup_core::secrets::save_secrets_store(&path, &vault_key, &store).map_err(
            |_| ControlError {
                code: "secrets.store_failed".to_string(),
                message: "Telegram session could not be saved.".to_string(),
                retryable: false,
                details: serde_json::json!({}),
            },
        )?;
    }
    Ok(
        serde_json::json!({"mode": "mtproto", "endpointId": endpoint.id, "chatId": endpoint.chat_id, "roundTripOk": true, "sampleObjectId": object_id}),
    )
}

async fn telegram_wait_chat(
    config_root: &std::path::Path,
    data_root: &std::path::Path,
    settings: &Settings,
    params: TelegramWaitChatParams,
) -> Result<serde_json::Value, ControlError> {
    let (storage, _store, _vault_key) = telegram_storage(
        config_root,
        data_root,
        settings,
        &params.endpoint_id,
        String::new(),
    )
    .await?;
    let timeout_seconds = params.timeout_seconds.clamp(1, 600);
    let chat = tokio::task::spawn_blocking(move || {
        storage.wait_for_chat(timeout_seconds, params.include_users)
    })
    .await
    .map_err(|error| {
        ControlError::unavailable(
            "Telegram chat discovery task failed",
            serde_json::json!({"reason": error.to_string()}),
        )
    })?
    .map_err(|_error| ControlError {
        code: "telegram.timeout".to_string(),
        message: "No Telegram chat was received before the listener expired.".to_string(),
        retryable: true,
        details: serde_json::json!({}),
    })?;
    Ok(
        serde_json::json!({"chat": {"kind": chat.kind, "title": chat.title, "username": chat.username, "peerId": chat.peer_id, "configChatId": chat.config_chat_id, "bootstrapHint": chat.bootstrap_hint}}),
    )
}

async fn refresh_control_settings_for_backup_enqueue(
    context: &ControlContext,
) -> Result<(), ControlError> {
    let config_root = context.config_root.clone();
    let settings = tokio::task::spawn_blocking(move || {
        let settings = televy_backup_core::config::load_settings_v2(&config_root)?;
        televy_backup_core::config::validate_settings_schema_v2(&settings)?;
        Ok::<_, televy_backup_core::Error>(settings)
    })
    .await
    .map_err(|error| {
        ControlError::unavailable(
            "settings reload task failed",
            serde_json::json!({ "error": error.to_string() }),
        )
    })?
    .map_err(|error| ControlError {
        code: "config.invalid".to_string(),
        message: error.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;

    *context.settings.write().await = settings;
    context
        .settings_reload_requested
        .store(true, Ordering::Release);
    Ok(())
}

fn handle_request(
    req: &ControlRequest,
    context: &ControlContext,
    settings: &Settings,
    logging: &LoggingStatusContext<'_>,
) -> ControlResponse {
    let config_root = &context.config_root;
    let status_state = &context.status_state;
    let backup_queue = &context.backup_queue;
    let backup_queue_notify = &context.backup_queue_notify;
    let lifecycle = &context.lifecycle;
    if req.type_ != "control.request" || req.id.trim().is_empty() || req.method.trim().is_empty() {
        return ControlResponse::err(
            req.id.clone(),
            ControlError::invalid_request(
                "invalid request envelope",
                serde_json::json!({
                    "type": req.type_,
                    "method": req.method,
                }),
            ),
        );
    }

    match req.method.as_str() {
        "logging.status" | "diagnostics.get" => {
            let configured = televy_backup_core::local_settings::resolve(config_root);
            let (has_running, external_logging) = status_state
                .lock()
                .ok()
                .map(|state| {
                    (
                        state.has_running(),
                        state.active_external_logging().cloned(),
                    )
                })
                .unwrap_or((false, None));
            let effective = if let Some(external_logging) = external_logging.as_ref() {
                external_logging
            } else if has_running {
                logging.runtime
            } else {
                &configured
            };
            let pending_level = (has_running
                && configured.configured_level != effective.configured_level)
                .then_some(configured.configured_level);
            let mut status = televy_backup_core::local_settings::status_with_log_usage(
                effective,
                pending_level,
                logging.data_root,
                true,
                logging.log_bytes,
                logging.managed_log_usage,
            );
            status.configured_level = configured.configured_level;
            status.retention = configured.retention;
            status.retention_prune_enabled = configured.retention_prune_enabled;
            status.configuration_error = configured.configuration_error;
            ControlResponse::ok(
                req.id.clone(),
                serde_json::to_value(status).unwrap_or_else(|_| serde_json::json!({})),
            )
        }
        "diagnostics.setLogLevel" => {
            let level = req
                .params
                .get("level")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    ControlError::invalid_request(
                        "diagnostics.setLogLevel requires level",
                        serde_json::json!({}),
                    )
                });
            match level.and_then(|value| {
                value
                    .parse::<televy_backup_core::local_settings::LogLevel>()
                    .map_err(|error| {
                        ControlError::invalid_request(
                            "invalid log level",
                            serde_json::json!({ "error": error }),
                        )
                    })
            }) {
                Ok(level) => {
                    match televy_backup_core::local_settings::update(config_root, |settings| {
                        settings.logging.level = level;
                    }) {
                        Ok(_) => {
                            ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true }))
                        }
                        Err(error) => ControlResponse::err(
                            req.id.clone(),
                            ControlError {
                                code: "diagnostics.save_failed".to_string(),
                                message: error.to_string(),
                                retryable: false,
                                details: serde_json::json!({}),
                            },
                        ),
                    }
                }
                Err(error) => ControlResponse::err(req.id.clone(), error),
            }
        }
        "diagnostics.setLogRetention" => {
            let max_total_gib = req
                .params
                .get("maxTotalGiB")
                .and_then(serde_json::Value::as_u64);
            let max_age_days = req
                .params
                .get("maxAgeDays")
                .and_then(serde_json::Value::as_u64);
            let retention = match (max_total_gib, max_age_days) {
                (Some(max_total_gib), Some(max_age_days)) => {
                    let value = televy_backup_core::local_settings::LogRetentionSettings {
                        max_total_gib: max_total_gib as u16,
                        max_age_days: max_age_days as u16,
                    };
                    value.validate().map(|_| value).map_err(|error| {
                        ControlError::invalid_request(
                            "invalid log retention",
                            serde_json::json!({ "error": error }),
                        )
                    })
                }
                _ => Err(ControlError::invalid_request(
                    "diagnostics.setLogRetention requires maxTotalGiB and maxAgeDays",
                    serde_json::json!({}),
                )),
            };
            match retention.and_then(|value| {
                televy_backup_core::local_settings::update(config_root, |settings| {
                    settings.logging.retention = value;
                })
                .map_err(|error| ControlError {
                    code: "diagnostics.save_failed".to_string(),
                    message: error.to_string(),
                    retryable: false,
                    details: serde_json::json!({}),
                })
            }) {
                Ok(_) => ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true })),
                Err(error) => ControlResponse::err(req.id.clone(), error),
            }
        }
        "daemon.stop" => {
            lifecycle.request_shutdown();
            ControlResponse::ok(
                req.id.clone(),
                serde_json::json!({ "shutdownRequested": true }),
            )
        }
        "operation.get" => operation_get(req),
        "restore.latest" => {
            let params: RestoreLatestParams = match serde_json::from_value(req.params.clone()) {
                Ok(params) => params,
                Err(error) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid restore.latest params",
                            serde_json::json!({"error": error.to_string()}),
                        ),
                    );
                }
            };
            ControlResponse::err(
                req.id.clone(),
                ControlError {
                    code: "restore.unavailable".to_string(),
                    message: "Restore is not available while the daemon has no remote snapshot operation.".to_string(),
                    retryable: true,
                    details: serde_json::json!({"targetId": params.target_id, "target": params.target}),
                },
            )
        }
        "vault.status" => match vault_status(config_root) {
            Ok(s) => ControlResponse::ok(
                req.id.clone(),
                serde_json::to_value(s).unwrap_or(serde_json::json!({})),
            ),
            Err(e) => ControlResponse::err(req.id.clone(), e),
        },
        "vault.ensure" => match vault_ensure(config_root) {
            Ok(s) => ControlResponse::ok(
                req.id.clone(),
                serde_json::to_value(s).unwrap_or(serde_json::json!({})),
            ),
            Err(e) => ControlResponse::err(req.id.clone(), e),
        },
        "backup.enqueue" => match backup_enqueue(
            config_root,
            settings,
            status_state,
            backup_queue,
            backup_queue_notify,
            req.params.clone(),
        ) {
            Ok(result) => ControlResponse::ok(
                req.id.clone(),
                serde_json::to_value(result).unwrap_or_else(|_| serde_json::json!({})),
            ),
            Err(error) => ControlResponse::err(req.id.clone(), error),
        },
        "backup.stop" => match backup_stop(
            status_state,
            backup_queue,
            backup_queue_notify,
            lifecycle,
            req.params.clone(),
        ) {
            Ok(result) => ControlResponse::ok(
                req.id.clone(),
                serde_json::to_value(result).unwrap_or_else(|_| serde_json::json!({})),
            ),
            Err(error) => ControlResponse::err(req.id.clone(), error),
        },
        "secrets.presence" => {
            let params: SecretsPresenceParams = match serde_json::from_value(req.params.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid params",
                            serde_json::json!({ "error": e.to_string() }),
                        ),
                    );
                }
            };

            match secrets_presence(config_root, settings, params.endpoint_id.as_deref()) {
                Ok(v) => ControlResponse::ok(req.id.clone(), v),
                Err(e) => ControlResponse::err(req.id.clone(), e),
            }
        }
        "secrets.setTelegramBotToken" => {
            let params: SecretsSetTelegramBotTokenParams =
                match serde_json::from_value(req.params.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return ControlResponse::err(
                            req.id.clone(),
                            ControlError::invalid_request(
                                "invalid params",
                                serde_json::json!({ "error": e.to_string() }),
                            ),
                        );
                    }
                };
            match secrets_set_telegram_bot_token(
                config_root,
                settings,
                &params.endpoint_id,
                &params.token,
            ) {
                Ok(()) => ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true })),
                Err(e) => ControlResponse::err(req.id.clone(), e),
            }
        }
        "secrets.setTelegramApiHash" => {
            let params: SecretsSetTelegramApiHashParams =
                match serde_json::from_value(req.params.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return ControlResponse::err(
                            req.id.clone(),
                            ControlError::invalid_request(
                                "invalid params",
                                serde_json::json!({ "error": e.to_string() }),
                            ),
                        );
                    }
                };
            match secrets_set_telegram_api_hash(config_root, settings, &params.api_hash) {
                Ok(()) => ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true })),
                Err(e) => ControlResponse::err(req.id.clone(), e),
            }
        }
        "secrets.clearTelegramMtprotoSession" => {
            let params: SecretsClearTelegramMtprotoSessionParams =
                match serde_json::from_value(req.params.clone()) {
                    Ok(p) => p,
                    Err(e) => {
                        return ControlResponse::err(
                            req.id.clone(),
                            ControlError::invalid_request(
                                "invalid params",
                                serde_json::json!({ "error": e.to_string() }),
                            ),
                        );
                    }
                };
            match secrets_clear_telegram_mtproto_session(config_root, settings, &params.endpoint_id)
            {
                Ok(()) => ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true })),
                Err(e) => ControlResponse::err(req.id.clone(), e),
            }
        }
        "status.taskStart" => {
            let params: StatusTaskStartParams = match serde_json::from_value(req.params.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid params",
                            serde_json::json!({ "error": e.to_string() }),
                        ),
                    );
                }
            };

            if televy_backup_core::status::ActiveTask::for_kind(&params.kind).is_none() {
                return ControlResponse::err(
                    req.id.clone(),
                    ControlError::invalid_request(
                        "unsupported task kind",
                        serde_json::json!({ "kind": params.kind }),
                    ),
                );
            }

            let mut st = match status_state.lock() {
                Ok(st) => st,
                Err(_) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::unavailable(
                            "status task admission unavailable",
                            serde_json::json!({ "targetId": params.target_id }),
                        ),
                    );
                }
            };
            match st.mark_external_run_start(
                &params.target_id,
                &params.task_id,
                &params.kind,
                params.process_id,
                params.logging,
            ) {
                Ok(()) => ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true })),
                Err(crate::ExternalTaskAdmissionError::TargetBusy(active_kind)) => {
                    ControlResponse::err(
                        req.id.clone(),
                        ControlError {
                            code: "target_busy".to_string(),
                            message: "target already has an active task".to_string(),
                            retryable: true,
                            details: serde_json::json!({
                                "targetId": params.target_id,
                                "activeKind": active_kind,
                            }),
                        },
                    )
                }
                Err(crate::ExternalTaskAdmissionError::TargetNotFound) => ControlResponse::err(
                    req.id.clone(),
                    ControlError {
                        code: "target_not_found".to_string(),
                        message: "target is not loaded by daemon".to_string(),
                        retryable: true,
                        details: serde_json::json!({ "targetId": params.target_id }),
                    },
                ),
                Err(crate::ExternalTaskAdmissionError::UnsupportedKind) => ControlResponse::err(
                    req.id.clone(),
                    ControlError::invalid_request(
                        "unsupported task kind",
                        serde_json::json!({ "kind": params.kind }),
                    ),
                ),
            }
        }
        "status.taskProgress" => {
            let params: StatusTaskProgressParams = match serde_json::from_value(req.params.clone())
            {
                Ok(p) => p,
                Err(e) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid params",
                            serde_json::json!({ "error": e.to_string() }),
                        ),
                    );
                }
            };

            if let Ok(mut st) = status_state.lock() {
                let p = TaskProgress {
                    phase: params.progress.phase,
                    files_total: params.progress.files_total,
                    files_done: params.progress.files_done,
                    source_files_total: params.progress.source_files_total,
                    source_bytes_total: params.progress.source_bytes_total,
                    source_bytes_need_upload_total: params.progress.source_bytes_need_upload_total,
                    chunks_total: params.progress.chunks_total,
                    chunks_done: params.progress.chunks_done,
                    bytes_read: params.progress.bytes_read,
                    upload_bytes_total: params.progress.upload_bytes_total,
                    bytes_uploaded_confirmed: params.progress.bytes_uploaded_confirmed,
                    bytes_uploaded_source: params.progress.bytes_uploaded_source,
                    bytes_uploaded: params.progress.bytes_uploaded,
                    net_bytes_uploaded: None,
                    bytes_downloaded: params.progress.bytes_downloaded,
                    net_bytes_downloaded: None,
                    bytes_deduped: params.progress.bytes_deduped,
                };
                st.on_external_progress(&params.target_id, &params.task_id, &params.kind, p);
            }
            ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true }))
        }
        "status.taskFinish" => {
            let params: StatusTaskFinishParams = match serde_json::from_value(req.params.clone()) {
                Ok(p) => p,
                Err(e) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::invalid_request(
                            "invalid params",
                            serde_json::json!({ "error": e.to_string() }),
                        ),
                    );
                }
            };

            if televy_backup_core::status::ActiveTask::for_kind(&params.kind).is_none() {
                return ControlResponse::err(
                    req.id.clone(),
                    ControlError::invalid_request(
                        "unsupported task kind",
                        serde_json::json!({ "kind": params.kind }),
                    ),
                );
            }
            if !matches!(params.state.as_str(), "succeeded" | "failed") {
                return ControlResponse::err(
                    req.id.clone(),
                    ControlError::invalid_request(
                        "unsupported task terminal state",
                        serde_json::json!({ "state": params.state }),
                    ),
                );
            }

            let mut st = match status_state.lock() {
                Ok(st) => st,
                Err(_) => {
                    return ControlResponse::err(
                        req.id.clone(),
                        ControlError::unavailable(
                            "status task completion unavailable",
                            serde_json::json!({ "targetId": params.target_id }),
                        ),
                    );
                }
            };
            match st.mark_external_run_finish(
                &params.target_id,
                &params.task_id,
                &params.kind,
                &params.state,
                params.error_code,
            ) {
                Ok(crate::ExternalTaskFinishOutcome::Applied) => ControlResponse::ok(
                    req.id.clone(),
                    serde_json::json!({ "ok": true, "acknowledged": true, "replayed": false }),
                ),
                Ok(crate::ExternalTaskFinishOutcome::IdempotentReplay) => ControlResponse::ok(
                    req.id.clone(),
                    serde_json::json!({ "ok": true, "acknowledged": true, "replayed": true }),
                ),
                Err(crate::ExternalTaskFinishError::TargetNotFound) => ControlResponse::err(
                    req.id.clone(),
                    ControlError {
                        code: "target_not_found".to_string(),
                        message: "target is not loaded by daemon".to_string(),
                        retryable: true,
                        details: serde_json::json!({ "targetId": params.target_id }),
                    },
                ),
                Err(crate::ExternalTaskFinishError::TaskNotOwned) => ControlResponse::err(
                    req.id.clone(),
                    ControlError {
                        code: "task_not_owned".to_string(),
                        message: "task does not own an active or matching terminal status"
                            .to_string(),
                        retryable: false,
                        details: serde_json::json!({
                            "targetId": params.target_id,
                            "taskId": params.task_id,
                            "kind": params.kind,
                        }),
                    },
                ),
            }
        }
        _ => ControlResponse::err(
            req.id.clone(),
            ControlError::method_not_found(
                "method not found",
                serde_json::json!({ "method": req.method }),
            ),
        ),
    }
}

fn backup_enqueue(
    config_root: &std::path::Path,
    settings: &Settings,
    status_state: &Arc<Mutex<crate::StatusRuntimeState>>,
    backup_queue: &Arc<Mutex<crate::BackupQueue>>,
    backup_queue_notify: &Arc<Notify>,
    raw_params: serde_json::Value,
) -> Result<BackupEnqueueResult, ControlError> {
    let target_ids_present = raw_params.get("targetIds").is_some();
    let params: BackupEnqueueParams = serde_json::from_value(raw_params).map_err(|error| {
        ControlError::invalid_request(
            "invalid params",
            serde_json::json!({ "error": error.to_string() }),
        )
    })?;

    let target_ids = match params.scope.as_str() {
        "allEnabled" => {
            if target_ids_present {
                return Err(ControlError::invalid_request(
                    "allEnabled must not include targetIds",
                    serde_json::json!({}),
                ));
            }
            settings
                .targets
                .iter()
                .filter(|target| target.enabled)
                .map(|target| target.id.clone())
                .collect::<Vec<_>>()
        }
        "targets" => {
            let target_ids = params.target_ids.ok_or_else(|| {
                ControlError::invalid_request(
                    "targets requires at least one non-empty targetId",
                    serde_json::json!({}),
                )
            })?;
            if target_ids.is_empty() || target_ids.iter().any(|id| id.trim().is_empty()) {
                return Err(ControlError::invalid_request(
                    "targets requires at least one non-empty targetId",
                    serde_json::json!({}),
                ));
            }
            let requested = target_ids.iter().collect::<std::collections::HashSet<_>>();
            let unknown = target_ids
                .iter()
                .filter(|id| !settings.targets.iter().any(|target| target.id == **id))
                .cloned()
                .collect::<Vec<_>>();
            if !unknown.is_empty() {
                return Err(ControlError::invalid_request(
                    "unknown targetId",
                    serde_json::json!({ "targetIds": unknown }),
                ));
            }
            settings
                .targets
                .iter()
                .filter(|target| requested.contains(&target.id))
                .map(|target| target.id.clone())
                .collect::<Vec<_>>()
        }
        _ => {
            return Err(ControlError::invalid_request(
                "scope must be allEnabled or targets",
                serde_json::json!({ "scope": params.scope }),
            ));
        }
    };

    if target_ids.is_empty() {
        return Err(ControlError {
            code: "backup.no_runnable_targets".to_string(),
            message: "no targets are available for backup".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }

    vault_ensure(config_root)?;
    if settings.telegram.mtproto.api_id <= 0
        || settings.telegram.mtproto.api_hash_key.trim().is_empty()
    {
        return Err(ControlError {
            code: "backup.telegram_api_unavailable".to_string(),
            message: "Telegram MTProto API credentials are unavailable".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }
    let presence = secrets_presence(config_root, settings, None)?;
    if !presence
        .get("masterKeyPresent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err(ControlError {
            code: "backup.master_key_unavailable".to_string(),
            message: "backup master key is unavailable".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }
    if !presence
        .get("telegramMtprotoApiHashPresent")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        return Err(ControlError {
            code: "backup.telegram_api_unavailable".to_string(),
            message: "Telegram MTProto API credentials are unavailable".to_string(),
            retryable: false,
            details: serde_json::json!({}),
        });
    }

    let target_order = settings
        .targets
        .iter()
        .map(|target| target.id.clone())
        .collect::<Vec<_>>();
    if let Ok(mut status) = status_state.lock() {
        status.add_missing_targets(settings);
    }
    let (batch_id, disposition, target_ids) = backup_queue
        .lock()
        .map_err(|_| ControlError::unavailable("backup queue unavailable", serde_json::json!({})))?
        .enqueue(target_ids, &target_order);
    crate::sync_backup_queue_memberships(backup_queue, status_state);
    backup_queue_notify.notify_one();

    Ok(BackupEnqueueResult {
        batch_id,
        disposition: disposition.to_string(),
        target_ids,
    })
}

fn backup_stop(
    status_state: &Arc<Mutex<crate::StatusRuntimeState>>,
    backup_queue: &Arc<Mutex<crate::BackupQueue>>,
    backup_queue_notify: &Arc<Notify>,
    lifecycle: &Arc<crate::DaemonLifecycle>,
    raw_params: serde_json::Value,
) -> Result<BackupStopResult, ControlError> {
    let _: BackupStopParams = serde_json::from_value(raw_params).map_err(|error| {
        ControlError::invalid_request(
            "invalid params",
            serde_json::json!({ "error": error.to_string() }),
        )
    })?;

    // The queue lock is acquired before lifecycle cancellation, matching the run-start handoff
    // in the main loop. A stop therefore cannot race a dequeued target into execution.
    let cleared_target_ids = backup_queue
        .lock()
        .map_err(|_| ControlError::unavailable("backup queue unavailable", serde_json::json!({})))?
        .clear();
    let cancellation_requested = lifecycle.request_backup_stop();
    crate::sync_backup_queue_memberships(backup_queue, status_state);
    backup_queue_notify.notify_one();

    Ok(BackupStopResult {
        cancellation_requested,
        cleared_target_ids,
    })
}

fn vault_status(config_root: &std::path::Path) -> Result<VaultStatusResult, ControlError> {
    let keychain_disabled = crate::keychain_disabled();
    let key_file_path = std::env::var("TELEVYBACKUP_VAULT_KEY_FILE")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| televy_backup_core::secrets::vault_key_file_path(config_root));

    if let Ok(b64) = std::env::var("TELEVYBACKUP_VAULT_KEY_B64") {
        let key_present = televy_backup_core::secrets::vault_key_from_base64(b64.trim()).is_ok();
        return Ok(VaultStatusResult {
            backend: "file".to_string(),
            key_present,
            keychain_disabled,
            vault_key_file_path: Some(key_file_path.display().to_string()),
        });
    }

    if keychain_disabled {
        match televy_backup_core::secrets::read_vault_key_file(&key_file_path) {
            Ok(_) => {
                return Ok(VaultStatusResult {
                    backend: "file".to_string(),
                    key_present: true,
                    keychain_disabled,
                    vault_key_file_path: Some(key_file_path.display().to_string()),
                });
            }
            Err(televy_backup_core::secrets::SecretsStoreError::Io(e))
                if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                return Err(ControlError {
                    code: "secrets.vault_key_file_io_failed".to_string(),
                    message: e.to_string(),
                    retryable: false,
                    details: serde_json::json!({ "path": key_file_path.display().to_string() }),
                });
            }
        }

        return Ok(VaultStatusResult {
            backend: "file".to_string(),
            key_present: false,
            keychain_disabled,
            vault_key_file_path: Some(key_file_path.display().to_string()),
        });
    }

    let key_present = crate::keychain_get_secret(televy_backup_core::secrets::VAULT_KEY_KEY)
        .ok()
        .flatten()
        .is_some();

    Ok(VaultStatusResult {
        backend: "keychain".to_string(),
        key_present,
        keychain_disabled,
        vault_key_file_path: None,
    })
}

fn vault_ensure(config_root: &std::path::Path) -> Result<VaultStatusResult, ControlError> {
    if crate::get_cached_vault_key().is_none() {
        // Keychain access may block waiting for user auth/permission. Avoid blocking Tokio worker
        // threads when possible.
        let res = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(crate::load_or_create_vault_key_uncached)
        } else {
            crate::load_or_create_vault_key_uncached()
        };

        match res {
            Ok(key) => {
                crate::set_cached_vault_key(key);
            }
            Err(e) => {
                return Err(ControlError {
                    code: "secrets.vault_unavailable".to_string(),
                    message: e.to_string(),
                    retryable: false,
                    details: serde_json::json!({}),
                });
            }
        }
    }
    vault_status(config_root)
}

fn secrets_presence(
    config_root: &std::path::Path,
    settings: &Settings,
    endpoint_id: Option<&str>,
) -> Result<serde_json::Value, ControlError> {
    if let Some(id) = endpoint_id
        && !settings.telegram_endpoints.iter().any(|e| e.id == id)
    {
        return Err(ControlError::invalid_request(
            "unknown endpoint_id",
            serde_json::json!({ "endpointId": id }),
        ));
    }

    let vault_key = crate::load_or_create_vault_key().map_err(|e| ControlError {
        code: "secrets.vault_unavailable".to_string(),
        message: e.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;

    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    let store = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
        .map_err(|e| ControlError {
            code: "secrets.store_failed".to_string(),
            message: e.to_string(),
            retryable: false,
            details: serde_json::json!({ "path": secrets_path.display().to_string() }),
        })?;

    let master_present = store.contains_key(crate::MASTER_KEY_KEY);

    let api_hash_present = store.contains_key(&settings.telegram.mtproto.api_hash_key);

    let mut bot_present_by_endpoint = serde_json::Map::<String, serde_json::Value>::new();
    let mut mtproto_session_present_by_endpoint =
        serde_json::Map::<String, serde_json::Value>::new();

    for ep in &settings.telegram_endpoints {
        if endpoint_id.is_some_and(|id| id != ep.id) {
            continue;
        }

        let bot_present = store.contains_key(&ep.bot_token_key);
        bot_present_by_endpoint.insert(ep.id.clone(), serde_json::Value::Bool(bot_present));

        let sess_present = store.contains_key(&ep.mtproto.session_key);
        mtproto_session_present_by_endpoint
            .insert(ep.id.clone(), serde_json::Value::Bool(sess_present));
    }

    Ok(serde_json::json!({
        "masterKeyPresent": master_present,
        "telegramMtprotoApiHashPresent": api_hash_present,
        "telegramBotTokenPresentByEndpoint": bot_present_by_endpoint,
        "telegramMtprotoSessionPresentByEndpoint": mtproto_session_present_by_endpoint,
    }))
}

fn secrets_set_telegram_bot_token(
    config_root: &std::path::Path,
    settings: &Settings,
    endpoint_id: &str,
    token: &str,
) -> Result<(), ControlError> {
    if token.trim().is_empty() {
        return Err(ControlError::invalid_request(
            "token is empty",
            serde_json::json!({}),
        ));
    }

    let ep = settings
        .telegram_endpoints
        .iter()
        .find(|e| e.id == endpoint_id)
        .ok_or_else(|| {
            ControlError::invalid_request(
                "unknown endpoint_id",
                serde_json::json!({ "endpointId": endpoint_id }),
            )
        })?;

    let vault_key = crate::load_or_create_vault_key().map_err(|e| ControlError {
        code: "secrets.vault_unavailable".to_string(),
        message: e.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;
    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    let mut store = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
        .map_err(|e| ControlError {
            code: "secrets.store_failed".to_string(),
            message: e.to_string(),
            retryable: false,
            details: serde_json::json!({ "path": secrets_path.display().to_string() }),
        })?;

    store.set(&ep.bot_token_key, token.trim());
    televy_backup_core::secrets::save_secrets_store(&secrets_path, &vault_key, &store).map_err(
        |e| ControlError {
            code: "secrets.store_failed".to_string(),
            message: e.to_string(),
            retryable: false,
            details: serde_json::json!({ "path": secrets_path.display().to_string() }),
        },
    )?;
    Ok(())
}

fn secrets_set_telegram_api_hash(
    config_root: &std::path::Path,
    settings: &Settings,
    api_hash: &str,
) -> Result<(), ControlError> {
    if api_hash.trim().is_empty() {
        return Err(ControlError::invalid_request(
            "api_hash is empty",
            serde_json::json!({}),
        ));
    }

    let vault_key = crate::load_or_create_vault_key().map_err(|e| ControlError {
        code: "secrets.vault_unavailable".to_string(),
        message: e.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;
    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    let mut store = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
        .map_err(|e| ControlError {
            code: "secrets.store_failed".to_string(),
            message: e.to_string(),
            retryable: false,
            details: serde_json::json!({ "path": secrets_path.display().to_string() }),
        })?;

    store.set(&settings.telegram.mtproto.api_hash_key, api_hash.trim());
    televy_backup_core::secrets::save_secrets_store(&secrets_path, &vault_key, &store).map_err(
        |e| ControlError {
            code: "secrets.store_failed".to_string(),
            message: e.to_string(),
            retryable: false,
            details: serde_json::json!({ "path": secrets_path.display().to_string() }),
        },
    )?;
    Ok(())
}

fn secrets_clear_telegram_mtproto_session(
    config_root: &std::path::Path,
    settings: &Settings,
    endpoint_id: &str,
) -> Result<(), ControlError> {
    let ep = settings
        .telegram_endpoints
        .iter()
        .find(|e| e.id == endpoint_id)
        .ok_or_else(|| {
            ControlError::invalid_request(
                "unknown endpoint_id",
                serde_json::json!({ "endpointId": endpoint_id }),
            )
        })?;

    let vault_key = crate::load_or_create_vault_key().map_err(|e| ControlError {
        code: "secrets.vault_unavailable".to_string(),
        message: e.to_string(),
        retryable: false,
        details: serde_json::json!({}),
    })?;
    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    let mut store = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
        .map_err(|e| ControlError {
            code: "secrets.store_failed".to_string(),
            message: e.to_string(),
            retryable: false,
            details: serde_json::json!({ "path": secrets_path.display().to_string() }),
        })?;

    let removed = store.remove(&ep.mtproto.session_key);
    if removed {
        televy_backup_core::secrets::save_secrets_store(&secrets_path, &vault_key, &store)
            .map_err(|e| ControlError {
                code: "secrets.store_failed".to_string(),
                message: e.to_string(),
                retryable: false,
                details: serde_json::json!({ "path": secrets_path.display().to_string() }),
            })?;
    }
    Ok(())
}

async fn write_json_line(
    w: &mut BufWriter<tokio::net::unix::OwnedWriteHalf>,
    v: &ControlResponse,
) -> std::io::Result<()> {
    let line = serde_json::to_string(v).map_err(|e| std::io::Error::other(e.to_string()))?;
    w.write_all(line.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use tokio::io::AsyncBufReadExt;

    use super::*;

    fn settings() -> Settings {
        let mut s = Settings::default();
        s.telegram_endpoints
            .push(televy_backup_core::config::TelegramEndpoint {
                id: "ep1".to_string(),
                mode: "mtproto".to_string(),
                chat_id: "-100".to_string(),
                bot_token_key: "telegram.bot_token.ep1".to_string(),
                mtproto: televy_backup_core::config::TelegramEndpointMtproto::default(),
                rate_limit: televy_backup_core::config::TelegramRateLimit::default(),
            });
        s.telegram_endpoints[0].mtproto.session_key = "telegram.mtproto.session.ep1".to_string();
        s.targets.push(televy_backup_core::config::Target {
            id: "t1".to_string(),
            source_path: "/tmp/source".to_string(),
            label: "test".to_string(),
            endpoint_id: "ep1".to_string(),
            enabled: true,
            schedule: None,
        });
        s
    }

    fn test_context(
        config_root: &std::path::Path,
        status_state: Arc<Mutex<crate::StatusRuntimeState>>,
        backup_queue: Arc<Mutex<crate::BackupQueue>>,
        backup_queue_notify: Arc<Notify>,
        lifecycle: Arc<crate::DaemonLifecycle>,
    ) -> ControlContext {
        let settings = Arc::new(RwLock::new(settings()));
        let data_root = config_root.to_path_buf();
        ControlContext {
            config_root: config_root.to_path_buf(),
            settings: settings.clone(),
            status_state,
            backup_queue,
            backup_queue_notify,
            settings_reload_requested: Arc::new(AtomicBool::new(false)),
            lifecycle,
            runtime_logging: Arc::new(RwLock::new(televy_backup_core::local_settings::resolve(
                config_root,
            ))),
            data_root: data_root.clone(),
            snapshot_inspection: Arc::new(
                crate::snapshot_inspection_ipc::SnapshotInspectionService::new(
                    config_root.to_path_buf(),
                    data_root,
                    settings,
                ),
            ),
        }
    }

    #[tokio::test]
    async fn backup_enqueue_refreshes_control_settings_and_requests_daemon_reload() {
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("config");
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let backup_queue = Arc::new(Mutex::new(crate::BackupQueue::default()));
        let backup_queue_notify = Arc::new(Notify::new());
        let lifecycle = Arc::new(crate::DaemonLifecycle::default());
        let context = test_context(
            &config_root,
            status_state,
            backup_queue,
            backup_queue_notify,
            lifecycle,
        );

        let mut imported = settings();
        imported.targets[0].id = "imported-target".to_string();
        imported.telegram_endpoints[0].mtproto.session_key =
            "telegram.mtproto.session.ep1".to_string();
        televy_backup_core::config::save_settings_v2(&config_root, &imported).unwrap();

        refresh_control_settings_for_backup_enqueue(&context)
            .await
            .unwrap();

        let current = context.settings.read().await;
        assert_eq!(current.targets[0].id, "imported-target");
        assert!(
            context
                .settings_reload_requested
                .swap(false, Ordering::AcqRel)
        );
    }

    #[tokio::test]
    async fn settings_control_ipc_revision_conflict_rejects_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("config");
        let initial = settings();
        televy_backup_core::config::save_settings_v2(&config_root, &initial).unwrap();
        let context = test_context(
            &config_root,
            Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
                &initial,
            ))),
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );

        let get = handle_settings_request(
            &ControlRequest::new("get-1", "settings.get", serde_json::json!({})),
            &context,
        )
        .await;
        assert!(get.ok);
        let value = get.result.unwrap();
        let decoded: SettingsGetResult = serde_json::from_value(value).unwrap();
        assert_eq!(
            decoded.revision,
            televy_backup_core::config::settings_revision(&initial).unwrap()
        );

        let mut changed = initial.clone();
        changed.retention.keep_last_snapshots += 1;
        let stale = handle_settings_request(
            &ControlRequest::new(
                "set-stale",
                "settings.set",
                serde_json::json!({"settings": changed, "expectedRevision": "stale"}),
            ),
            &context,
        )
        .await;
        assert!(!stale.ok);
        assert_eq!(stale.error.unwrap().code, "settings.revision_conflict");

        let set = handle_settings_request(
            &ControlRequest::new(
                "set-1",
                "settings.set",
                serde_json::json!({"settings": changed, "expectedRevision": decoded.revision}),
            ),
            &context,
        )
        .await;
        assert!(set.ok);
        let saved = televy_backup_core::config::load_settings_v2(&config_root).unwrap();
        assert_eq!(
            saved.retention.keep_last_snapshots,
            initial.retention.keep_last_snapshots + 1
        );
    }

    #[tokio::test]
    async fn operation_get_returns_terminal_result() {
        let operation_id = operation_start();
        spawn_operation(operation_id.clone(), async {
            Ok(serde_json::json!({"ok": true}))
        });

        for _ in 0..20 {
            let response = operation_get(&ControlRequest::new(
                "operation-status",
                "operation.get",
                serde_json::json!({"operationId": operation_id}),
            ));
            if response
                .result
                .as_ref()
                .and_then(|value| value.get("state"))
                .and_then(serde_json::Value::as_str)
                == Some("succeeded")
            {
                assert_eq!(response.result.unwrap()["result"]["ok"], true);
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("operation did not reach a terminal state");
    }

    #[test]
    fn unfinished_bundle_transaction_restores_settings() {
        let dir = tempfile::tempdir().unwrap();
        let original = settings();
        televy_backup_core::config::save_settings_v2(dir.path(), &original).unwrap();
        begin_bundle_transaction(dir.path()).unwrap();

        let mut changed = original.clone();
        changed.retention.keep_last_snapshots += 1;
        televy_backup_core::config::save_settings_v2(dir.path(), &changed).unwrap();

        assert!(recover_bundle_transaction(dir.path()).unwrap());
        let restored = televy_backup_core::config::load_settings_v2(dir.path()).unwrap();
        assert_eq!(
            restored.retention.keep_last_snapshots,
            original.retention.keep_last_snapshots
        );
        assert!(!dir.path().join(BUNDLE_TXN_MARKER).exists());
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("ipc").join("control.sock");
        let cfg_root = dir.path().join("cfg");
        std::fs::create_dir_all(&cfg_root).unwrap();

        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let runtime_logging = Arc::new(RwLock::new(televy_backup_core::local_settings::resolve(
            &cfg_root,
        )));
        let control_settings = Arc::new(RwLock::new(settings()));
        let data_root = dir.path().join("data");
        let _server = spawn_control_ipc_server(
            socket_path.clone(),
            ControlContext {
                config_root: cfg_root,
                settings: control_settings.clone(),
                status_state,
                backup_queue: Arc::new(Mutex::new(crate::BackupQueue::default())),
                backup_queue_notify: Arc::new(Notify::new()),
                settings_reload_requested: Arc::new(AtomicBool::new(false)),
                lifecycle: Arc::new(crate::DaemonLifecycle::default()),
                runtime_logging,
                data_root: data_root.clone(),
                snapshot_inspection: Arc::new(
                    crate::snapshot_inspection_ipc::SnapshotInspectionService::new(
                        dir.path().join("cfg"),
                        data_root,
                        control_settings,
                    ),
                ),
            },
        )
        .unwrap();

        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let (r, mut w) = stream.into_split();
        let mut r = tokio::io::BufReader::new(r).lines();

        let req = ControlRequest::new("1", "unknown.method", serde_json::json!({}));
        let line = serde_json::to_string(&req).unwrap() + "\n";
        w.write_all(line.as_bytes()).await.unwrap();
        w.flush().await.unwrap();

        let resp_line = r.next_line().await.unwrap().unwrap();
        let resp: ControlResponse = serde_json::from_str(&resp_line).unwrap();
        assert!(!resp.ok);
        assert_eq!(
            resp.error.as_ref().unwrap().code,
            "control.method_not_found"
        );
    }

    #[test]
    fn daemon_stop_requests_lifecycle_shutdown() {
        let lifecycle = Arc::new(crate::DaemonLifecycle::default());
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let runtime_logging =
            televy_backup_core::local_settings::resolve(std::path::Path::new("/tmp"));
        let backup_queue = Arc::new(Mutex::new(crate::BackupQueue::default()));
        let backup_queue_notify = Arc::new(Notify::new());
        let context = test_context(
            std::path::Path::new("/tmp"),
            status_state.clone(),
            backup_queue.clone(),
            backup_queue_notify.clone(),
            lifecycle.clone(),
        );
        let response = handle_request(
            &ControlRequest::new("1", "daemon.stop", serde_json::json!({})),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: std::path::Path::new("/tmp"),
                log_bytes: None,
                managed_log_usage: None,
            },
        );

        assert!(response.ok);
        assert!(lifecycle.is_shutdown_requested());
    }

    #[test]
    fn backup_stop_cancels_active_task_and_clears_manual_queue() {
        let lifecycle = Arc::new(crate::DaemonLifecycle::default());
        let active_task = lifecycle.begin_task();
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let runtime_logging =
            televy_backup_core::local_settings::resolve(std::path::Path::new("/tmp"));
        let backup_queue = Arc::new(Mutex::new(crate::BackupQueue::default()));
        let backup_queue_notify = Arc::new(Notify::new());
        backup_queue
            .lock()
            .unwrap()
            .enqueue(vec!["t1".to_string()], &["t1".to_string()]);
        backup_queue.lock().unwrap().start_next_target();
        crate::sync_backup_queue_memberships(&backup_queue, &status_state);

        let context = test_context(
            std::path::Path::new("/tmp"),
            status_state.clone(),
            backup_queue.clone(),
            backup_queue_notify,
            lifecycle,
        );
        let response = handle_request(
            &ControlRequest::new("1", "backup.stop", serde_json::json!({})),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: std::path::Path::new("/tmp"),
                log_bytes: None,
                managed_log_usage: None,
            },
        );

        assert!(response.ok);
        let result: BackupStopResult = serde_json::from_value(response.result.unwrap()).unwrap();
        assert!(result.cancellation_requested);
        assert_eq!(result.cleared_target_ids, vec!["t1"]);
        assert!(active_task.is_cancelled());
        assert!(!backup_queue.lock().unwrap().has_work());
        assert!(
            status_state.lock().unwrap().targets["t1"]
                .backup_queue
                .is_none()
        );
    }

    #[test]
    fn status_task_start_returns_target_busy_without_replacing_activity() {
        let config_root = std::path::Path::new("/tmp");
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let context = test_context(
            config_root,
            status_state.clone(),
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );
        let runtime_logging = televy_backup_core::local_settings::resolve(config_root);
        let logging = LoggingStatusContext {
            runtime: &runtime_logging,
            data_root: config_root,
            log_bytes: None,
            managed_log_usage: None,
        };

        let started = handle_request(
            &ControlRequest::new(
                "restore",
                "status.taskStart",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1"
                }),
            ),
            &context,
            &settings(),
            &logging,
        );
        assert!(started.ok);

        let rejected = handle_request(
            &ControlRequest::new(
                "verify",
                "status.taskStart",
                serde_json::json!({
                    "taskId": "verify-2",
                    "kind": "verify",
                    "targetId": "t1"
                }),
            ),
            &context,
            &settings(),
            &logging,
        );
        let error = rejected.error.expect("target busy error");
        assert_eq!(error.code, "target_busy");
        assert!(error.retryable);
        assert_eq!(error.details["activeKind"], "restore");
        assert_eq!(
            status_state.lock().unwrap().targets["t1"]
                .active_task
                .as_ref()
                .map(|task| task.kind.as_str()),
            Some("restore")
        );
    }

    #[test]
    fn status_task_start_rejects_targets_missing_from_runtime_state() {
        let config_root = std::path::Path::new("/tmp");
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let context = test_context(
            config_root,
            status_state,
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );
        let runtime_logging = televy_backup_core::local_settings::resolve(config_root);
        let response = handle_request(
            &ControlRequest::new(
                "restore",
                "status.taskStart",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "missing"
                }),
            ),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: config_root,
                log_bytes: None,
                managed_log_usage: None,
            },
        );

        assert!(!response.ok);
        let error = response.error.expect("target not found error");
        assert_eq!(error.code, "target_not_found");
        assert!(error.retryable);
    }

    #[test]
    fn status_task_start_fails_closed_when_status_state_is_poisoned() {
        let config_root = std::path::Path::new("/tmp");
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let poisoned = status_state.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned
                .lock()
                .expect("status state should lock before poisoning");
            panic!("poison status state");
        })
        .join();

        let context = test_context(
            config_root,
            status_state,
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );
        let runtime_logging = televy_backup_core::local_settings::resolve(config_root);
        let response = handle_request(
            &ControlRequest::new(
                "restore",
                "status.taskStart",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1"
                }),
            ),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: config_root,
                log_bytes: None,
                managed_log_usage: None,
            },
        );

        assert!(!response.ok);
        assert_eq!(
            response.error.expect("admission unavailable error").code,
            "control.unavailable"
        );
    }

    #[test]
    fn status_task_finish_preserves_external_failure_code() {
        let config_root = std::path::Path::new("/tmp");
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let context = test_context(
            config_root,
            status_state.clone(),
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );
        let runtime_logging = televy_backup_core::local_settings::resolve(config_root);
        let logging = LoggingStatusContext {
            runtime: &runtime_logging,
            data_root: config_root,
            log_bytes: None,
            managed_log_usage: None,
        };

        let started = handle_request(
            &ControlRequest::new(
                "restore",
                "status.taskStart",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1"
                }),
            ),
            &context,
            &settings(),
            &logging,
        );
        assert!(started.ok);

        let finished = handle_request(
            &ControlRequest::new(
                "restore-finish",
                "status.taskFinish",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1",
                    "state": "failed",
                    "errorCode": "restore.network_failed"
                }),
            ),
            &context,
            &settings(),
            &logging,
        );
        assert!(finished.ok);
        let target = &status_state.lock().unwrap().targets["t1"];
        assert_eq!(target.state, "failed");
        assert!(target.active_task.is_none());
        assert_eq!(
            target
                .last_run
                .as_ref()
                .and_then(|run| run.error_code.as_deref()),
            Some("restore.network_failed")
        );
    }

    #[test]
    fn status_task_finish_acknowledges_only_applied_or_matching_replayed_terminal_state() {
        let config_root = std::path::Path::new("/tmp");
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let context = test_context(
            config_root,
            status_state.clone(),
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );
        let runtime_logging = televy_backup_core::local_settings::resolve(config_root);
        let logging = LoggingStatusContext {
            runtime: &runtime_logging,
            data_root: config_root,
            log_bytes: None,
            managed_log_usage: None,
        };

        let started = handle_request(
            &ControlRequest::new(
                "restore",
                "status.taskStart",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1"
                }),
            ),
            &context,
            &settings(),
            &logging,
        );
        assert!(started.ok);

        let params = serde_json::json!({
            "taskId": "restore-1",
            "kind": "restore",
            "targetId": "t1",
            "state": "failed",
            "errorCode": "restore.network_failed"
        });
        let applied = handle_request(
            &ControlRequest::new("restore-finish", "status.taskFinish", params.clone()),
            &context,
            &settings(),
            &logging,
        );
        assert!(applied.ok);
        assert_eq!(applied.result.unwrap()["replayed"], false);

        let replayed = handle_request(
            &ControlRequest::new("restore-finish-retry", "status.taskFinish", params.clone()),
            &context,
            &settings(),
            &logging,
        );
        assert!(replayed.ok);
        assert_eq!(replayed.result.unwrap()["replayed"], true);

        let restarted_status_state = Arc::new(Mutex::new(
            crate::StatusRuntimeState::from_settings(&settings()),
        ));
        let restarted_context = test_context(
            config_root,
            restarted_status_state,
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );
        let after_restart = handle_request(
            &ControlRequest::new(
                "restore-finish-after-restart",
                "status.taskFinish",
                params.clone(),
            ),
            &restarted_context,
            &settings(),
            &logging,
        );
        assert!(!after_restart.ok);
        assert_eq!(
            after_restart
                .error
                .expect("restarted daemon must reject an unowned terminal state")
                .code,
            "task_not_owned"
        );

        let stale = handle_request(
            &ControlRequest::new(
                "restore-finish-stale",
                "status.taskFinish",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1",
                    "state": "succeeded"
                }),
            ),
            &context,
            &settings(),
            &logging,
        );
        assert!(!stale.ok);
        assert_eq!(
            stale.error.expect("stale terminal ownership error").code,
            "task_not_owned"
        );
    }

    #[test]
    fn status_task_finish_fails_closed_when_status_state_is_poisoned() {
        let config_root = std::path::Path::new("/tmp");
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let poisoned = status_state.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoned
                .lock()
                .expect("status state should lock before poisoning");
            panic!("poison status state");
        })
        .join();

        let context = test_context(
            config_root,
            status_state,
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );
        let runtime_logging = televy_backup_core::local_settings::resolve(config_root);
        let response = handle_request(
            &ControlRequest::new(
                "restore-finish",
                "status.taskFinish",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1",
                    "state": "failed",
                    "errorCode": "restore.network_failed"
                }),
            ),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: config_root,
                log_bytes: None,
                managed_log_usage: None,
            },
        );

        assert!(!response.ok);
        assert_eq!(
            response.error.expect("completion unavailable error").code,
            "control.unavailable"
        );
    }

    #[test]
    fn status_task_finish_rejects_unknown_terminal_state_without_releasing_target() {
        let config_root = std::path::Path::new("/tmp");
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let context = test_context(
            config_root,
            status_state.clone(),
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            Arc::new(crate::DaemonLifecycle::default()),
        );
        let runtime_logging = televy_backup_core::local_settings::resolve(config_root);
        let logging = LoggingStatusContext {
            runtime: &runtime_logging,
            data_root: config_root,
            log_bytes: None,
            managed_log_usage: None,
        };

        let started = handle_request(
            &ControlRequest::new(
                "restore",
                "status.taskStart",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1"
                }),
            ),
            &context,
            &settings(),
            &logging,
        );
        assert!(started.ok);

        let finished = handle_request(
            &ControlRequest::new(
                "restore-finish",
                "status.taskFinish",
                serde_json::json!({
                    "taskId": "restore-1",
                    "kind": "restore",
                    "targetId": "t1",
                    "state": "cancelled"
                }),
            ),
            &context,
            &settings(),
            &logging,
        );
        assert!(!finished.ok);
        assert_eq!(
            finished.error.expect("invalid state error").code,
            "control.invalid_request"
        );
        assert_eq!(status_state.lock().unwrap().targets["t1"].state, "running");
    }

    #[test]
    fn backup_enqueue_rejects_mixed_scope_before_admission_checks() {
        let lifecycle = Arc::new(crate::DaemonLifecycle::default());
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let runtime_logging =
            televy_backup_core::local_settings::resolve(std::path::Path::new("/tmp"));
        let backup_queue = Arc::new(Mutex::new(crate::BackupQueue::default()));
        let backup_queue_notify = Arc::new(Notify::new());
        let context = test_context(
            std::path::Path::new("/tmp"),
            status_state.clone(),
            backup_queue.clone(),
            backup_queue_notify.clone(),
            lifecycle.clone(),
        );
        let response = handle_request(
            &ControlRequest::new(
                "1",
                "backup.enqueue",
                serde_json::json!({
                    "scope": "allEnabled",
                    "targetIds": ["t1"]
                }),
            ),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: std::path::Path::new("/tmp"),
                log_bytes: None,
                managed_log_usage: None,
            },
        );

        assert!(!response.ok);
        let error = response.error.expect("structured backup enqueue error");
        assert_eq!(error.code, "control.invalid_request");
        assert!(error.message.contains("must not include targetIds"));
    }

    #[test]
    fn backup_enqueue_rejects_unknown_params_before_admission_checks() {
        let lifecycle = Arc::new(crate::DaemonLifecycle::default());
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let runtime_logging =
            televy_backup_core::local_settings::resolve(std::path::Path::new("/tmp"));
        let context = test_context(
            std::path::Path::new("/tmp"),
            status_state,
            Arc::new(Mutex::new(crate::BackupQueue::default())),
            Arc::new(Notify::new()),
            lifecycle,
        );
        let response = handle_request(
            &ControlRequest::new(
                "1",
                "backup.enqueue",
                serde_json::json!({
                    "scope": "allEnabled",
                    "unexpected": true
                }),
            ),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: std::path::Path::new("/tmp"),
                log_bytes: None,
                managed_log_usage: None,
            },
        );

        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("control.invalid_request")
        );
    }

    #[test]
    fn backup_enqueue_rejects_empty_target_ids_for_all_enabled_scope() {
        let lifecycle = Arc::new(crate::DaemonLifecycle::default());
        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let runtime_logging =
            televy_backup_core::local_settings::resolve(std::path::Path::new("/tmp"));
        let backup_queue = Arc::new(Mutex::new(crate::BackupQueue::default()));
        let backup_queue_notify = Arc::new(Notify::new());
        let context = test_context(
            std::path::Path::new("/tmp"),
            status_state,
            backup_queue,
            backup_queue_notify,
            lifecycle,
        );
        let response = handle_request(
            &ControlRequest::new(
                "1",
                "backup.enqueue",
                serde_json::json!({
                    "scope": "allEnabled",
                    "targetIds": []
                }),
            ),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: std::path::Path::new("/tmp"),
                log_bytes: None,
                managed_log_usage: None,
            },
        );

        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("control.invalid_request")
        );

        let response = handle_request(
            &ControlRequest::new(
                "2",
                "backup.enqueue",
                serde_json::json!({
                    "scope": "allEnabled",
                    "targetIds": null
                }),
            ),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: std::path::Path::new("/tmp"),
                log_bytes: None,
                managed_log_usage: None,
            },
        );
        assert!(!response.ok);
        assert_eq!(
            response.error.as_ref().map(|error| error.code.as_str()),
            Some("control.invalid_request")
        );
    }

    #[test]
    fn logging_status_reports_runtime_filter_and_pending_level() {
        let dir = tempfile::tempdir().unwrap();
        let config_root = dir.path().join("config");
        let data_root = dir.path().join("data");
        let runtime_logging =
            televy_backup_core::local_settings::resolve_from(&config_root, None, None);
        televy_backup_core::local_settings::save(
            &config_root,
            &televy_backup_core::local_settings::LocalSettings {
                version: 1,
                logging: televy_backup_core::local_settings::LoggingSettings {
                    level: televy_backup_core::local_settings::LogLevel::Debug,
                    ..Default::default()
                },
            },
        )
        .unwrap();

        let status_state = Arc::new(Mutex::new(crate::StatusRuntimeState::from_settings(
            &settings(),
        )));
        let backup_queue = Arc::new(Mutex::new(crate::BackupQueue::default()));
        let backup_queue_notify = Arc::new(Notify::new());
        let lifecycle = Arc::new(crate::DaemonLifecycle::default());
        let context = test_context(
            &config_root,
            status_state.clone(),
            backup_queue.clone(),
            backup_queue_notify.clone(),
            lifecycle,
        );
        status_state.lock().unwrap().mark_run_start("t1");
        let response = handle_request(
            &ControlRequest::new("1", "logging.status", serde_json::json!({})),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: &data_root,
                log_bytes: Some(0),
                managed_log_usage: None,
            },
        );

        let status: televy_backup_core::local_settings::LoggingStatus =
            serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(status.effective_level, "normal");
        assert_eq!(
            status.configured_level,
            televy_backup_core::local_settings::LogLevel::Debug
        );
        assert_eq!(
            status.pending_level,
            Some(televy_backup_core::local_settings::LogLevel::Debug)
        );
        assert!(status.daemon_available);

        let external_logging =
            televy_backup_core::local_settings::resolve_from(&config_root, Some("debug"), None);
        {
            let mut status = status_state.lock().unwrap();
            status.mark_run_finish_success("t1", 0.0, 0, 0, 0);
            status
                .mark_external_run_start("t1", "cli-task", "restore", None, Some(external_logging))
                .unwrap();
        }
        let response = handle_request(
            &ControlRequest::new("external", "logging.status", serde_json::json!({})),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: &data_root,
                log_bytes: Some(0),
                managed_log_usage: None,
            },
        );
        let status: televy_backup_core::local_settings::LoggingStatus =
            serde_json::from_value(response.result.unwrap()).unwrap();
        assert_eq!(status.effective_level, "debug");
        assert_eq!(status.overridden_by.as_deref(), Some("TELEVYBACKUP_LOG"));
        status_state
            .lock()
            .unwrap()
            .mark_external_run_finish("t1", "cli-task", "restore", "succeeded", None)
            .unwrap();
        status_state.lock().unwrap().mark_run_start("t1");

        std::fs::write(
            televy_backup_core::local_settings::local_settings_path(&config_root),
            "[logging]\nlevel = 'debug'\n",
        )
        .unwrap();
        let response = handle_request(
            &ControlRequest::new("2", "logging.status", serde_json::json!({})),
            &context,
            &settings(),
            &LoggingStatusContext {
                runtime: &runtime_logging,
                data_root: &data_root,
                log_bytes: Some(0),
                managed_log_usage: None,
            },
        );
        let status: televy_backup_core::local_settings::LoggingStatus =
            serde_json::from_value(response.result.unwrap()).unwrap();
        assert!(status.configuration_error.is_some());
        assert_eq!(status.effective_level, "normal");
    }
}
