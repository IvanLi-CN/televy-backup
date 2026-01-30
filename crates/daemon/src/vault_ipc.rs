use std::path::PathBuf;

use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, oneshot};

#[derive(Debug, serde::Deserialize)]
#[serde(tag = "type")]
enum VaultIpcRequest {
    #[serde(rename = "vault.get_or_create")]
    VaultGetOrCreate,

    #[serde(rename = "keychain.get")]
    KeychainGet { key: String },

    #[serde(rename = "keychain.delete")]
    KeychainDelete { key: String },
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct VaultIpcResponse {
    ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    vault_key_b64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    deleted: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

pub struct VaultIpcServerHandle {
    socket_path: PathBuf,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl VaultIpcServerHandle {
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

impl Drop for VaultIpcServerHandle {
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

pub fn spawn_vault_ipc_server(socket_path: PathBuf) -> std::io::Result<VaultIpcServerHandle> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Err(e) = std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            {
                tracing::warn!(
                    event = "vault.ipc_permissions_failed",
                    error = %e,
                    path = %parent.display(),
                    "vault.ipc_permissions_failed"
                );
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
        if let Err(e) = std::fs::set_permissions(&socket_path, std::fs::Permissions::from_mode(0o600))
        {
            tracing::warn!(
                event = "vault.ipc_permissions_failed",
                error = %e,
                path = %socket_path.display(),
                "vault.ipc_permissions_failed"
            );
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
                                event = "vault.ipc_accept_failed",
                                error = %e,
                                path = %socket_path.display(),
                                "vault.ipc_accept_failed"
                            );
                            continue;
                        }
                    };

                    let mut shutdown = shutdown_broadcast.subscribe();
                    tokio::spawn(async move {
                        let _ = handle_vault_ipc_client(stream, &mut shutdown).await;
                    });
                }
            }
        }
    });

    Ok(VaultIpcServerHandle {
        socket_path: handle_socket_path,
        shutdown_tx: Some(shutdown_tx),
        task: Some(task),
    })
}

async fn handle_vault_ipc_client(
    stream: UnixStream,
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
            _ = shutdown.recv() => {
                return Ok(());
            }
        }
    }

    if buf.is_empty() {
        return Ok(());
    }

    if buf.len() > MAX_REQUEST_LINE_BYTES {
        write_json_line(
            &mut w,
            VaultIpcResponse {
                ok: false,
                vault_key_b64: None,
                value: None,
                deleted: None,
                error: Some("request too large".to_string()),
            },
        )
        .await?;
        return Ok(());
    }

    let line = match String::from_utf8(buf) {
        Ok(s) => s,
        Err(_) => {
            write_json_line(
                &mut w,
                VaultIpcResponse {
                    ok: false,
                    vault_key_b64: None,
                    value: None,
                    deleted: None,
                    error: Some("invalid utf-8".to_string()),
                },
            )
            .await?;
            return Ok(());
        }
    };

    let req: VaultIpcRequest = match serde_json::from_str(line.trim_end()) {
        Ok(x) => x,
        Err(e) => {
            write_json_line(
                &mut w,
                VaultIpcResponse {
                    ok: false,
                    vault_key_b64: None,
                    value: None,
                    deleted: None,
                    error: Some(format!("invalid json: {e}")),
                },
            )
            .await?;
            return Ok(());
        }
    };

    let resp = match req {
        VaultIpcRequest::VaultGetOrCreate => match crate::load_or_create_vault_key() {
            Ok(key) => VaultIpcResponse {
                ok: true,
                vault_key_b64: Some(televy_backup_core::secrets::vault_key_to_base64(&key)),
                value: None,
                deleted: None,
                error: None,
            },
            Err(e) => VaultIpcResponse {
                ok: false,
                vault_key_b64: None,
                value: None,
                deleted: None,
                error: Some(e.to_string()),
            },
        },
        VaultIpcRequest::KeychainGet { key } => match crate::keychain_get_secret(&key) {
            Ok(v) => VaultIpcResponse {
                ok: true,
                vault_key_b64: None,
                value: v,
                deleted: None,
                error: None,
            },
            Err(e) => VaultIpcResponse {
                ok: false,
                vault_key_b64: None,
                value: None,
                deleted: None,
                error: Some(e.to_string()),
            },
        },
        VaultIpcRequest::KeychainDelete { key } => match crate::keychain_delete_secret(&key) {
            Ok(deleted) => VaultIpcResponse {
                ok: true,
                vault_key_b64: None,
                value: None,
                deleted: Some(deleted),
                error: None,
            },
            Err(e) => VaultIpcResponse {
                ok: false,
                vault_key_b64: None,
                value: None,
                deleted: None,
                error: Some(e.to_string()),
            },
        },
    };

    write_json_line(&mut w, resp).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn creates_socket_and_accepts_connections() {
        unsafe {
            std::env::set_var("TELEVYBACKUP_DISABLE_KEYCHAIN", "1");
        }
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("ipc").join("vault.sock");

        let server = spawn_vault_ipc_server(socket_path.clone()).unwrap();

        let _ = UnixStream::connect(&socket_path).await.unwrap();

        server.shutdown().await;
    }
}

async fn write_json_line<W: tokio::io::AsyncWrite + Unpin>(
    w: &mut BufWriter<W>,
    v: VaultIpcResponse,
) -> std::io::Result<()> {
    let line = serde_json::to_string(&v)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    w.write_all(line.as_bytes()).await?;
    w.write_all(b"\n").await?;
    w.flush().await?;
    Ok(())
}
