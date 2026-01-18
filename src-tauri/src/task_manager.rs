use std::collections::HashMap;

use serde::Serialize;
use tauri::Emitter;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use crate::rpc::RpcError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskStateEvent {
    pub task_id: String,
    pub kind: String,
    pub state: String,
    pub error: Option<TaskError>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskError {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressEvent {
    pub task_id: String,
    pub phase: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub files_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks_total: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chunks_done: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_read: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_uploaded: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bytes_deduped: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct TaskStatus {
    pub kind: String,
    pub state: String,
    pub phase: String,
    pub progress: TaskProgressEvent,
    pub cancel: CancellationToken,
    pub snapshot_id: Option<String>,
}

#[derive(Default)]
pub struct TaskManager {
    inner: Mutex<HashMap<String, TaskStatus>>,
}

impl TaskManager {
    pub async fn start_task(
        &self,
        app: &tauri::AppHandle,
        kind: &str,
        initial_phase: &str,
    ) -> (String, CancellationToken) {
        let task_id = uuid::Uuid::new_v4().to_string();
        let cancel = CancellationToken::new();
        let status = TaskStatus {
            kind: kind.to_string(),
            state: "queued".to_string(),
            phase: initial_phase.to_string(),
            progress: TaskProgressEvent {
                task_id: task_id.clone(),
                phase: initial_phase.to_string(),
                ..TaskProgressEvent::default()
            },
            cancel: cancel.clone(),
            snapshot_id: None,
        };

        self.inner.lock().await.insert(task_id.clone(), status);
        emit_state(app, &task_id, kind, "queued", None);
        (task_id, cancel)
    }

    pub async fn set_running(&self, app: &tauri::AppHandle, task_id: &str) {
        if let Some(s) = self.inner.lock().await.get_mut(task_id) {
            s.state = "running".to_string();
            emit_state(app, task_id, &s.kind, "running", None);
        }
    }

    pub async fn set_snapshot_id(&self, task_id: &str, snapshot_id: &str) {
        if let Some(s) = self.inner.lock().await.get_mut(task_id) {
            s.snapshot_id = Some(snapshot_id.to_string());
        }
    }

    pub async fn update_progress(
        &self,
        app: &tauri::AppHandle,
        task_id: &str,
        p: TaskProgressEvent,
    ) {
        if let Some(s) = self.inner.lock().await.get_mut(task_id) {
            s.progress = p.clone();
            s.phase = p.phase.clone();
        }
        emit_progress(app, &p);
    }

    pub async fn finish_ok(&self, app: &tauri::AppHandle, task_id: &str) {
        if let Some(s) = self.inner.lock().await.get_mut(task_id) {
            s.state = "succeeded".to_string();
            emit_state(app, task_id, &s.kind, "succeeded", None);
        }
    }

    pub async fn finish_err(&self, app: &tauri::AppHandle, task_id: &str, err: &RpcError) {
        if let Some(s) = self.inner.lock().await.get_mut(task_id) {
            let state = if err.code == "task.cancelled" {
                "cancelled"
            } else {
                "failed"
            };
            s.state = state.to_string();
            emit_state(
                app,
                task_id,
                &s.kind,
                state,
                Some(TaskError {
                    code: err.code.clone(),
                    message: err.message.clone(),
                }),
            );
        }
    }

    pub async fn status(&self, task_id: &str) -> Option<(String, String, TaskProgressEvent)> {
        self.inner
            .lock()
            .await
            .get(task_id)
            .map(|s| (s.state.clone(), s.phase.clone(), s.progress.clone()))
    }

    pub async fn cancel(&self, task_id: &str) -> bool {
        if let Some(s) = self.inner.lock().await.get(task_id) {
            s.cancel.cancel();
            return true;
        }
        false
    }

    pub async fn ensure_no_running(&self, kind: &str) -> Result<(), RpcError> {
        let m = self.inner.lock().await;
        let any = m.values().any(|s| s.kind == kind && s.state == "running");
        if any {
            return Err(RpcError::new(
                "task.already_running",
                "task already running".to_string(),
            ));
        }
        Ok(())
    }
}

fn emit_state(
    app: &tauri::AppHandle,
    task_id: &str,
    kind: &str,
    state: &str,
    error: Option<TaskError>,
) {
    let payload = TaskStateEvent {
        task_id: task_id.to_string(),
        kind: kind.to_string(),
        state: state.to_string(),
        error,
    };
    let _ = app.emit("task:state", payload);
}

fn emit_progress(app: &tauri::AppHandle, payload: &TaskProgressEvent) {
    let _ = app.emit("task:progress", payload.clone());
}
