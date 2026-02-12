use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{RwLock, broadcast, oneshot};

use televy_backup_core::control::{
    ControlError, ControlRequest, ControlResponse, SecretsClearTelegramMtprotoSessionParams,
    SecretsPresenceParams, SecretsSetTelegramApiHashParams, SecretsSetTelegramBotTokenParams,
    VaultStatusResult,
};

type Settings = televy_backup_core::config::SettingsV2;

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

pub fn spawn_control_ipc_server(
    socket_path: PathBuf,
    config_root: PathBuf,
    settings: Arc<RwLock<Settings>>,
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
                    let config_root = config_root.clone();
                    let settings = settings.clone();
                    tokio::spawn(async move {
                        let _ = handle_control_ipc_client(stream, &config_root, settings, &mut shutdown).await;
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
    config_root: &std::path::Path,
    settings: Arc<RwLock<Settings>>,
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

    let resp = {
        let settings = settings.read().await;
        handle_request(&req, config_root, &settings)
    };
    write_json_line(&mut w, &resp).await?;
    Ok(())
}

fn handle_request(
    req: &ControlRequest,
    config_root: &std::path::Path,
    settings: &Settings,
) -> ControlResponse {
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
        _ => ControlResponse::err(
            req.id.clone(),
            ControlError::method_not_found(
                "method not found",
                serde_json::json!({ "method": req.method }),
            ),
        ),
    }
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
    if crate::VAULT_KEY_CACHE.get().is_none() {
        // Keychain access may block waiting for user auth/permission. Avoid blocking Tokio worker
        // threads when possible.
        let res = if tokio::runtime::Handle::try_current().is_ok() {
            tokio::task::block_in_place(|| crate::load_or_create_vault_key_uncached())
        } else {
            crate::load_or_create_vault_key_uncached()
        };

        match res {
            Ok(key) => {
                let _ = crate::VAULT_KEY_CACHE.set(key);
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
        s
    }

    #[tokio::test]
    async fn unknown_method_returns_method_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("ipc").join("control.sock");
        let cfg_root = dir.path().join("cfg");
        std::fs::create_dir_all(&cfg_root).unwrap();

        let _server = spawn_control_ipc_server(
            socket_path.clone(),
            cfg_root.clone(),
            Arc::new(RwLock::new(settings())),
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
}
