use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{Notify, RwLock, broadcast, oneshot};

use televy_backup_core::TaskProgress;
use televy_backup_core::control::{
    BackupEnqueueParams, BackupEnqueueResult, ControlError, ControlRequest, ControlResponse,
    SecretsClearTelegramMtprotoSessionParams, SecretsPresenceParams,
    SecretsSetTelegramApiHashParams, SecretsSetTelegramBotTokenParams, StatusTaskFinishParams,
    StatusTaskProgressParams, StatusTaskStartParams, VaultStatusResult,
};

type Settings = televy_backup_core::config::SettingsV2;

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

    let (log_bytes, managed_log_usage) = if req.method == "logging.status" {
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
        "logging.status" => {
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
        "daemon.stop" => {
            lifecycle.request_shutdown();
            ControlResponse::ok(
                req.id.clone(),
                serde_json::json!({ "shutdownRequested": true }),
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

            if let Ok(mut st) = status_state.lock() {
                st.mark_external_run_start(&params.target_id, &params.task_id, params.logging);
            }
            ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true }))
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
                st.on_external_progress(&params.target_id, &params.task_id, p);
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

            if let Ok(mut st) = status_state.lock() {
                st.mark_external_run_finish(&params.target_id, &params.task_id);
            }
            ControlResponse::ok(req.id.clone(), serde_json::json!({ "ok": true }))
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
        ControlContext {
            config_root: config_root.to_path_buf(),
            settings: Arc::new(RwLock::new(settings())),
            status_state,
            backup_queue,
            backup_queue_notify,
            settings_reload_requested: Arc::new(AtomicBool::new(false)),
            lifecycle,
            runtime_logging: Arc::new(RwLock::new(televy_backup_core::local_settings::resolve(
                config_root,
            ))),
            data_root: config_root.to_path_buf(),
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
        let _server = spawn_control_ipc_server(
            socket_path.clone(),
            ControlContext {
                config_root: cfg_root,
                settings: Arc::new(RwLock::new(settings())),
                status_state,
                backup_queue: Arc::new(Mutex::new(crate::BackupQueue::default())),
                backup_queue_notify: Arc::new(Notify::new()),
                settings_reload_requested: Arc::new(AtomicBool::new(false)),
                lifecycle: Arc::new(crate::DaemonLifecycle::default()),
                runtime_logging,
                data_root: dir.path().join("data"),
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
        status_state.lock().unwrap().mark_external_run_start(
            "t1",
            "cli-task",
            Some(external_logging),
        );
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
            .mark_external_run_finish("t1", "cli-task");
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
