use std::path::PathBuf;
use std::sync::Arc;

use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::{broadcast, oneshot};
use tokio::time::{Duration, sleep};

use televy_backup_core::status::StatusSnapshot;

pub struct StatusIpcServerHandle {
    socket_path: PathBuf,
    shutdown_tx: Option<oneshot::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl StatusIpcServerHandle {
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

impl Drop for StatusIpcServerHandle {
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

pub fn spawn_status_ipc_server(
    socket_path: PathBuf,
    snapshot_fn: Arc<dyn Fn() -> (StatusSnapshot, bool) + Send + Sync>,
) -> std::io::Result<StatusIpcServerHandle> {
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::remove_file(&socket_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let listener = UnixListener::bind(&socket_path)?;
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
                                event = "status.ipc_accept_failed",
                                error = %e,
                                path = %socket_path.display(),
                                "status.ipc_accept_failed"
                            );
                            sleep(Duration::from_millis(200)).await;
                            continue;
                        }
                    };

                    let snapshot_fn = snapshot_fn.clone();
                    let mut shutdown = shutdown_broadcast.subscribe();
                    tokio::spawn(async move {
                        let _ = handle_status_ipc_client(stream, snapshot_fn, &mut shutdown).await;
                    });
                }
            }
        }
    });

    Ok(StatusIpcServerHandle {
        socket_path: handle_socket_path,
        shutdown_tx: Some(shutdown_tx),
        task: Some(task),
    })
}

async fn handle_status_ipc_client(
    stream: UnixStream,
    snapshot_fn: Arc<dyn Fn() -> (StatusSnapshot, bool) + Send + Sync>,
    shutdown: &mut broadcast::Receiver<()>,
) -> std::io::Result<()> {
    let mut w = BufWriter::new(stream);

    loop {
        let (snapshot, has_running) = snapshot_fn();
        let line = serde_json::to_string(&snapshot)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;

        if w.write_all(line.as_bytes()).await.is_err() {
            return Ok(());
        }
        if w.write_all(b"\n").await.is_err() {
            return Ok(());
        }
        if w.flush().await.is_err() {
            return Ok(());
        }

        let tick = if has_running {
            Duration::from_millis(100)
        } else {
            Duration::from_secs(1)
        };

        tokio::select! {
            _ = sleep(tick) => {}
            _ = shutdown.recv() => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

    use tokio::io::AsyncBufReadExt;
    use tokio::time::{Instant, timeout};

    use televy_backup_core::status::{
        Counter, GlobalStatus, Rate, StatusSource, TargetState,
    };

    use super::*;

    fn snapshot(generated_at: u64) -> StatusSnapshot {
        StatusSnapshot {
            type_: "status.snapshot".to_string(),
            schema_version: 1,
            generated_at,
            source: StatusSource {
                kind: "daemon".to_string(),
                detail: Some("test".to_string()),
            },
            global: GlobalStatus {
                up: Rate { bytes_per_second: None },
                down: Rate { bytes_per_second: None },
                up_total: Counter { bytes: None },
                down_total: Counter { bytes: None },
                ui_uptime_seconds: None,
            },
            targets: vec![TargetState {
                target_id: "t1".to_string(),
                label: None,
                source_path: "/tmp".to_string(),
                endpoint_id: "ep".to_string(),
                enabled: true,
                state: "idle".to_string(),
                running_since: None,
                up: Rate { bytes_per_second: None },
                up_total: Counter { bytes: None },
                progress: None,
                last_run: None,
                extra: Default::default(),
            }],
            extra: Default::default(),
        }
    }

    #[tokio::test]
    async fn sends_first_snapshot_within_500ms() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("ipc").join("status.sock");

        let running = Arc::new(AtomicBool::new(false));
        let seq = Arc::new(AtomicU64::new(1));
        let snapshot_fn = {
            let running = running.clone();
            let seq = seq.clone();
            Arc::new(move || {
                let n = seq.fetch_add(1, Ordering::SeqCst);
                (snapshot(n), running.load(Ordering::SeqCst))
            })
        };

        let server = spawn_status_ipc_server(socket_path.clone(), snapshot_fn).unwrap();

        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let mut lines = tokio::io::BufReader::new(stream).lines();

        let started = Instant::now();
        let line = timeout(Duration::from_millis(500), lines.next_line())
            .await
            .expect("first line timeout")
            .expect("read error")
            .expect("EOF");

        assert!(started.elapsed() <= Duration::from_millis(500));

        let v: serde_json::Value = serde_json::from_str(&line).unwrap();
        assert_eq!(v["type"], "status.snapshot");

        server.shutdown().await;
    }

    #[tokio::test]
    async fn idle_cadence_is_not_spammy() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("ipc").join("status.sock");

        let running = Arc::new(AtomicBool::new(false));
        let seq = Arc::new(AtomicU64::new(1));
        let snapshot_fn = {
            let running = running.clone();
            let seq = seq.clone();
            Arc::new(move || {
                let n = seq.fetch_add(1, Ordering::SeqCst);
                (snapshot(n), running.load(Ordering::SeqCst))
            })
        };

        let server = spawn_status_ipc_server(socket_path.clone(), snapshot_fn).unwrap();

        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let mut lines = tokio::io::BufReader::new(stream).lines();

        let _ = lines.next_line().await.unwrap().unwrap();

        // Idle cadence is 1Hz; we should not get a second line too quickly.
        assert!(timeout(Duration::from_millis(200), lines.next_line()).await.is_err());
        let _ = timeout(Duration::from_millis(1500), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();

        server.shutdown().await;
    }

    #[tokio::test]
    async fn running_cadence_is_fast_enough() {
        let dir = tempfile::tempdir().unwrap();
        let socket_path = dir.path().join("ipc").join("status.sock");

        let running = Arc::new(AtomicBool::new(true));
        let seq = Arc::new(AtomicU64::new(1));
        let snapshot_fn = {
            let running = running.clone();
            let seq = seq.clone();
            Arc::new(move || {
                let n = seq.fetch_add(1, Ordering::SeqCst);
                (snapshot(n), running.load(Ordering::SeqCst))
            })
        };

        let server = spawn_status_ipc_server(socket_path.clone(), snapshot_fn).unwrap();

        let stream = UnixStream::connect(&socket_path).await.unwrap();
        let mut lines = tokio::io::BufReader::new(stream).lines();

        let _ = lines.next_line().await.unwrap().unwrap();
        let _ = timeout(Duration::from_millis(400), lines.next_line())
            .await
            .expect("expected another line quickly")
            .unwrap()
            .unwrap();

        server.shutdown().await;
    }
}
