use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use base64::Engine;
use chrono::{Datelike, Timelike};
use sqlx::Row;
use televy_backup_core::status::{
    ActiveTask, BackupQueueMembership, Counter, GlobalStatus, Progress, Rate, StatusSnapshot,
    StatusSource, StatusWriteOptions, TargetRunSummary, TargetState, now_unix_ms,
    status_ipc_socket_path, status_json_path, write_status_snapshot_json_atomic_with_options,
};
use televy_backup_core::{
    BackupConfig, BackupOptions, ChunkingConfig, SourceQuickStats, TelegramMtProtoStorage,
    TelegramMtProtoStorageConfig,
};
use televy_backup_core::{ProgressSink, Storage, TaskProgress};
use televy_backup_core::{bootstrap, config as settings_config};
use tokio::sync::{Notify, RwLock};
use tokio::time::{Duration, sleep};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

mod control_ipc;
mod status_ipc;
mod vault_ipc;

#[derive(Default)]
pub(crate) struct DaemonLifecycle {
    shutdown: CancellationToken,
    active_task: Mutex<Option<CancellationToken>>,
}

impl DaemonLifecycle {
    fn begin_task(&self) -> CancellationToken {
        let token = self.shutdown.child_token();
        *self
            .active_task
            .lock()
            .expect("daemon lifecycle lock poisoned") = Some(token.clone());
        token
    }

    fn finish_task(&self) {
        *self
            .active_task
            .lock()
            .expect("daemon lifecycle lock poisoned") = None;
    }

    pub(crate) fn request_shutdown(&self) {
        self.shutdown.cancel();
        if let Some(task) = self
            .active_task
            .lock()
            .expect("daemon lifecycle lock poisoned")
            .as_ref()
        {
            task.cancel();
        }
    }

    pub(crate) fn request_backup_stop(&self) -> bool {
        let active_task = self
            .active_task
            .lock()
            .expect("daemon lifecycle lock poisoned")
            .clone();
        if let Some(task) = active_task {
            task.cancel();
            true
        } else {
            false
        }
    }

    fn is_shutdown_requested(&self) -> bool {
        self.shutdown.is_cancelled()
    }

    fn shutdown_token(&self) -> CancellationToken {
        self.shutdown.clone()
    }
}

#[derive(Default, Clone)]
struct TargetScheduleState {
    last_hourly: Option<(i32, u32, u32, u32)>, // year, month, day, hour
    last_daily: Option<(i32, u32, u32)>,       // year, month, day
}

#[derive(Debug, Clone, Copy)]
enum ScheduleSlot {
    Hourly((i32, u32, u32, u32)),
    Daily((i32, u32, u32)),
    Manual,
}

#[derive(Debug, Clone)]
pub(crate) struct BackupBatch {
    id: String,
    target_ids: VecDeque<String>,
    started: bool,
}

#[derive(Debug, Default)]
pub(crate) struct BackupQueue {
    active: Option<BackupBatch>,
    pending: Option<BackupBatch>,
}

impl BackupQueue {
    pub(crate) fn enqueue(
        &mut self,
        target_ids: Vec<String>,
        target_order: &[String],
    ) -> (String, &'static str, Vec<String>) {
        if self.active.is_none() {
            let id = format!("bch_{}", Uuid::new_v4());
            self.active = Some(BackupBatch {
                id: id.clone(),
                target_ids: target_ids.into(),
                started: false,
            });
            Self::sort_targets_in_config_order(
                self.active.as_mut().expect("active batch was just created"),
                target_order,
            );
            let target_ids = self
                .active
                .as_ref()
                .expect("active batch was just created")
                .target_ids
                .iter()
                .cloned()
                .collect();
            return (id, "accepted", target_ids);
        }

        let (batch, disposition) = if self.active.as_ref().is_some_and(|batch| !batch.started) {
            (
                self.active.as_mut().expect("active batch checked above"),
                "coalesced",
            )
        } else if self.pending.is_some() {
            (
                self.pending.as_mut().expect("pending batch checked above"),
                "coalesced",
            )
        } else {
            let id = format!("bch_{}", Uuid::new_v4());
            self.pending = Some(BackupBatch {
                id,
                target_ids: VecDeque::new(),
                started: false,
            });
            (
                self.pending
                    .as_mut()
                    .expect("pending batch was just created"),
                "accepted",
            )
        };

        for target_id in target_ids {
            if !batch
                .target_ids
                .iter()
                .any(|existing| existing == &target_id)
            {
                batch.target_ids.push_back(target_id);
            }
        }
        Self::sort_targets_in_config_order(batch, target_order);

        (
            batch.id.clone(),
            disposition,
            batch.target_ids.iter().cloned().collect(),
        )
    }

    fn sort_targets_in_config_order(batch: &mut BackupBatch, target_order: &[String]) {
        batch.target_ids.make_contiguous().sort_by_key(|target_id| {
            target_order
                .iter()
                .position(|configured_id| configured_id == target_id)
                .unwrap_or(usize::MAX)
        });
    }

    pub(crate) fn start_next_target(&mut self) -> Option<String> {
        let batch = self.active.as_mut()?;
        batch.started = true;
        batch.target_ids.front().cloned()
    }

    pub(crate) fn complete_active_target(&mut self, target_id: &str) {
        let Some(active) = self.active.as_mut() else {
            return;
        };
        if active
            .target_ids
            .front()
            .is_some_and(|current| current == target_id)
        {
            active.target_ids.pop_front();
        }
        if active.target_ids.is_empty() {
            self.active = self.pending.take();
        }
    }

    pub(crate) fn clear(&mut self) -> Vec<String> {
        let mut target_ids = Vec::new();
        for batch in [&self.active, &self.pending].into_iter().flatten() {
            for target_id in &batch.target_ids {
                if !target_ids.contains(target_id) {
                    target_ids.push(target_id.clone());
                }
            }
        }
        self.active = None;
        self.pending = None;
        target_ids
    }

    fn active_target_matches(&self, target_id: &str) -> bool {
        self.active
            .as_ref()
            .and_then(|batch| batch.target_ids.front())
            .is_some_and(|current| current == target_id)
    }

    pub(crate) fn has_work(&self) -> bool {
        self.active.is_some() || self.pending.is_some()
    }

    fn memberships(&self) -> HashMap<String, BackupQueueMembership> {
        let mut memberships = HashMap::new();
        if let Some(active) = &self.active {
            for target_id in &active.target_ids {
                memberships
                    .entry(target_id.clone())
                    .or_insert_with(BackupQueueMembership::default)
                    .active_batch_id = Some(active.id.clone());
            }
        }
        if let Some(pending) = &self.pending {
            for target_id in &pending.target_ids {
                memberships
                    .entry(target_id.clone())
                    .or_insert_with(BackupQueueMembership::default)
                    .pending_batch_id = Some(pending.id.clone());
            }
        }
        memberships
    }
}

pub(crate) fn sync_backup_queue_memberships(
    queue: &Arc<Mutex<BackupQueue>>,
    status_state: &Arc<Mutex<StatusRuntimeState>>,
) {
    let memberships = queue
        .lock()
        .map(|queue| queue.memberships())
        .unwrap_or_default();
    if let Ok(mut status) = status_state.lock() {
        status.set_backup_queue_memberships(&memberships);
    }
}

fn start_next_queued_target(
    queue: &Arc<Mutex<BackupQueue>>,
    settings_reload_requested: &AtomicBool,
) -> Option<String> {
    queue.lock().ok().and_then(|mut queue| {
        // Enqueue sets this flag before it acquires the queue lock. Checking it while
        // holding the same lock keeps a just-admitted target from running against stale settings.
        (!settings_reload_requested.load(Ordering::Acquire))
            .then(|| queue.start_next_target())
            .flatten()
    })
}

fn complete_backup_queue_target(
    queue: &Arc<Mutex<BackupQueue>>,
    status_state: &Arc<Mutex<StatusRuntimeState>>,
    target_id: &str,
) {
    if let Ok(mut queue) = queue.lock() {
        queue.complete_active_target(target_id);
    }
    sync_backup_queue_memberships(queue, status_state);
}

fn fail_backup_queue_target(
    queue: &Arc<Mutex<BackupQueue>>,
    status_state: &Arc<Mutex<StatusRuntimeState>>,
    target_id: &str,
    error_code: &str,
) {
    backup_task_failed(status_state, target_id, 0.0, error_code);
    complete_backup_queue_target(queue, status_state, target_id);
}

fn backup_task_succeeded(
    status_state: &Arc<Mutex<StatusRuntimeState>>,
    target_id: &str,
    duration_seconds: f64,
    files_indexed: u64,
    bytes_uploaded: u64,
    bytes_deduped: u64,
) {
    if let Ok(mut status) = status_state.lock() {
        status.mark_run_finish_success(
            target_id,
            duration_seconds,
            files_indexed,
            bytes_uploaded,
            bytes_deduped,
        );
    }
}

fn backup_task_failed(
    status_state: &Arc<Mutex<StatusRuntimeState>>,
    target_id: &str,
    duration_seconds: f64,
    error_code: &str,
) {
    if let Ok(mut status) = status_state.lock() {
        status.mark_run_finish_failure(target_id, duration_seconds, error_code.to_string());
    }
}

fn backup_task_cancelled(
    status_state: &Arc<Mutex<StatusRuntimeState>>,
    target_id: &str,
    duration_seconds: f64,
) {
    if let Ok(mut status) = status_state.lock() {
        status.mark_run_finish_cancelled(target_id, duration_seconds);
    }
}

#[derive(Debug, Clone)]
struct TargetRuntime {
    target_id: String,
    label: Option<String>,
    source_path: String,
    endpoint_id: String,
    enabled: bool,

    state: String, // "idle" | "running" | "failed"
    running_since: Option<u64>,
    progress: Option<Progress>,
    last_run: Option<TargetRunSummary>,
    active_task: Option<ActiveTask>,
    backup_queue: Option<BackupQueueMembership>,

    // When a CLI-run task reports progress to the daemon for UI status purposes, we keep the
    // current task id here so stale updates don't clobber newer runs.
    external_task_id: Option<String>,
    external_process_id: Option<u32>,
    external_last_report_at: Option<Instant>,
    external_logging: Option<televy_backup_core::local_settings::ResolvedLogging>,

    up_bps: Option<u64>,
    up_total_bytes: Option<u64>,
    up_rate: ByteRateWindow,

    down_bps: Option<u64>,
    down_total_bytes: Option<u64>,
    down_rate: ByteRateWindow,
}

#[derive(Debug)]
struct StatusRuntimeState {
    target_order: Vec<String>,
    targets: HashMap<String, TargetRuntime>,
}

const EXTERNAL_TASK_REPORT_TIMEOUT: Duration = Duration::from_secs(120);

#[cfg(unix)]
fn is_process_alive(pid: u32) -> bool {
    if pid == 0 || pid > i32::MAX as u32 {
        return false;
    }
    if unsafe { libc::kill(pid as libc::pid_t, 0) } == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(not(unix))]
fn is_process_alive(_pid: u32) -> bool {
    true
}

impl StatusRuntimeState {
    fn from_settings(settings: &settings_config::SettingsV2) -> Self {
        let mut target_order = Vec::new();
        let mut targets = HashMap::new();
        for t in &settings.targets {
            target_order.push(t.id.clone());
            targets.insert(
                t.id.clone(),
                TargetRuntime {
                    target_id: t.id.clone(),
                    label: if t.label.trim().is_empty() {
                        None
                    } else {
                        Some(t.label.clone())
                    },
                    source_path: t.source_path.clone(),
                    endpoint_id: t.endpoint_id.clone(),
                    enabled: t.enabled,
                    state: "idle".to_string(),
                    running_since: None,
                    progress: None,
                    last_run: None,
                    active_task: None,
                    backup_queue: None,
                    external_task_id: None,
                    external_process_id: None,
                    external_last_report_at: None,
                    external_logging: None,
                    up_bps: None,
                    up_total_bytes: None,
                    up_rate: ByteRateWindow::default(),
                    down_bps: None,
                    down_total_bytes: None,
                    down_rate: ByteRateWindow::default(),
                },
            );
        }
        Self {
            target_order,
            targets,
        }
    }

    fn apply_settings(&mut self, settings: &settings_config::SettingsV2) {
        let mut target_order = Vec::new();
        let mut targets = HashMap::new();

        for t in &settings.targets {
            target_order.push(t.id.clone());

            let mut rt = self.targets.get(&t.id).cloned().unwrap_or(TargetRuntime {
                target_id: t.id.clone(),
                label: None,
                source_path: t.source_path.clone(),
                endpoint_id: t.endpoint_id.clone(),
                enabled: t.enabled,
                state: "idle".to_string(),
                running_since: None,
                progress: None,
                last_run: None,
                active_task: None,
                backup_queue: None,
                external_task_id: None,
                external_process_id: None,
                external_last_report_at: None,
                external_logging: None,
                up_bps: None,
                up_total_bytes: None,
                up_rate: ByteRateWindow::default(),
                down_bps: None,
                down_total_bytes: None,
                down_rate: ByteRateWindow::default(),
            });

            rt.label = if t.label.trim().is_empty() {
                None
            } else {
                Some(t.label.clone())
            };
            rt.source_path = t.source_path.clone();
            rt.endpoint_id = t.endpoint_id.clone();
            rt.enabled = t.enabled;

            targets.insert(t.id.clone(), rt);
        }

        self.target_order = target_order;
        self.targets = targets;
    }

    pub(crate) fn add_missing_targets(&mut self, settings: &settings_config::SettingsV2) {
        for t in &settings.targets {
            if self.targets.contains_key(&t.id) {
                continue;
            }
            self.target_order.push(t.id.clone());
            self.targets.insert(
                t.id.clone(),
                TargetRuntime {
                    target_id: t.id.clone(),
                    label: if t.label.trim().is_empty() {
                        None
                    } else {
                        Some(t.label.clone())
                    },
                    source_path: t.source_path.clone(),
                    endpoint_id: t.endpoint_id.clone(),
                    enabled: t.enabled,
                    state: "idle".to_string(),
                    running_since: None,
                    progress: None,
                    last_run: None,
                    active_task: None,
                    backup_queue: None,
                    external_task_id: None,
                    external_process_id: None,
                    external_last_report_at: None,
                    external_logging: None,
                    up_bps: None,
                    up_total_bytes: None,
                    up_rate: ByteRateWindow::default(),
                    down_bps: None,
                    down_total_bytes: None,
                    down_rate: ByteRateWindow::default(),
                },
            );
        }
    }

    #[cfg(test)]
    fn mark_run_start(&mut self, target_id: &str) {
        self.mark_run_start_with_phase(target_id, "running");
    }

    #[cfg(test)]
    fn mark_backup_run_start(&mut self, target_id: &str) {
        self.mark_run_start_with_phase(target_id, "connecting");
    }

    fn try_mark_backup_run_start(&mut self, target_id: &str, phase: &str) -> bool {
        if self.target_is_busy(target_id) {
            return false;
        }
        self.mark_run_start_with_phase(target_id, phase);
        true
    }

    fn mark_run_start_with_phase(&mut self, target_id: &str, phase: &str) {
        let Some(t) = self.targets.get_mut(target_id) else {
            return;
        };
        t.external_task_id = None;
        t.external_process_id = None;
        t.external_last_report_at = None;
        t.external_logging = None;
        t.active_task = ActiveTask::for_kind("backup");
        t.state = "running".to_string();
        let now = now_unix_ms();
        t.running_since = Some(now);
        t.progress = Some(Progress {
            phase: phase.to_string(),
            files_total: None,
            files_done: None,
            source_files_total: None,
            source_bytes_total: None,
            source_bytes_need_upload_total: None,
            chunks_total: None,
            chunks_done: None,
            bytes_read: None,
            upload_bytes_total: None,
            bytes_uploaded_confirmed: Some(0),
            bytes_uploaded_source: Some(0),
            bytes_uploaded: Some(0),
            bytes_downloaded: Some(0),
            bytes_deduped: Some(0),
        });
        t.up_total_bytes = Some(0);
        t.up_bps = Some(0);
        t.up_rate.reset(Instant::now(), 0);

        t.down_total_bytes = Some(0);
        t.down_bps = Some(0);
        t.down_rate.reset(Instant::now(), 0);
    }

    fn mark_external_run_start(
        &mut self,
        target_id: &str,
        task_id: &str,
        kind: &str,
        process_id: Option<u32>,
        logging: Option<televy_backup_core::local_settings::ResolvedLogging>,
    ) -> Result<(), String> {
        let activity = ActiveTask::for_kind(kind)
            .ok_or_else(|| format!("unsupported external task kind: {kind}"))?;
        let Some(t) = self.targets.get_mut(target_id) else {
            return Ok(());
        };

        if t.external_task_id.as_deref() == Some(task_id) {
            return Ok(());
        }
        if t.active_task.is_some() || t.state == "running" {
            return Err(t
                .active_task
                .as_ref()
                .map(|active| active.kind.clone())
                .unwrap_or_else(|| "unknown".to_string()));
        }

        let now = now_unix_ms();
        t.external_task_id = Some(task_id.to_string());
        t.external_process_id = process_id.filter(|pid| *pid > 0);
        t.external_last_report_at = Some(Instant::now());
        t.external_logging = logging;
        t.active_task = Some(activity);
        t.state = "running".to_string();
        t.running_since = Some(now);
        t.progress = Some(Progress {
            phase: "running".to_string(),
            files_total: None,
            files_done: None,
            source_files_total: None,
            source_bytes_total: None,
            source_bytes_need_upload_total: None,
            chunks_total: None,
            chunks_done: None,
            bytes_read: Some(0),
            upload_bytes_total: None,
            bytes_uploaded_confirmed: Some(0),
            bytes_uploaded_source: Some(0),
            bytes_uploaded: Some(0),
            bytes_downloaded: Some(0),
            bytes_deduped: Some(0),
        });

        // Reset upload baselines so status sampling can compute rates cleanly for CLI runs.
        t.up_total_bytes = Some(0);
        t.up_bps = Some(0);
        t.up_rate.reset(Instant::now(), 0);

        t.down_total_bytes = Some(0);
        t.down_bps = Some(0);
        t.down_rate.reset(Instant::now(), 0);
        Ok(())
    }

    fn on_external_progress(
        &mut self,
        target_id: &str,
        task_id: &str,
        kind: &str,
        p: TaskProgress,
    ) {
        let Some(t) = self.targets.get_mut(target_id) else {
            return;
        };

        // Ignore stale updates from an earlier task.
        match t.external_task_id.as_deref() {
            Some(active) if active != task_id => return,
            None if t.active_task.is_some() => return,
            None => {
                let Some(activity) = ActiveTask::for_kind(kind) else {
                    return;
                };
                t.external_task_id = Some(task_id.to_string());
                t.active_task = Some(activity);
            }
            _ => {}
        }

        t.external_last_report_at = Some(Instant::now());

        if t.state != "running" {
            t.state = "running".to_string();
        }
        if t.running_since.is_none() {
            t.running_since = Some(now_unix_ms());
        }

        t.progress = Some(Progress {
            phase: p.phase,
            files_total: p.files_total,
            files_done: p.files_done,
            source_files_total: p.source_files_total,
            source_bytes_total: p.source_bytes_total,
            source_bytes_need_upload_total: p.source_bytes_need_upload_total,
            chunks_total: p.chunks_total,
            chunks_done: p.chunks_done,
            bytes_read: p.bytes_read,
            upload_bytes_total: p.upload_bytes_total,
            bytes_uploaded_confirmed: p.bytes_uploaded_confirmed,
            bytes_uploaded_source: p.bytes_uploaded_source,
            bytes_uploaded: p.bytes_uploaded,
            bytes_downloaded: p.bytes_downloaded,
            bytes_deduped: p.bytes_deduped,
        });

        // Prefer payload bytes for "last 1s" transfer rates.
        //
        // Wire-byte counters (socket writes/reads) can get ahead due to kernel buffering and cause
        // brief spikes followed by misleading "0" periods while the OS drains buffers. Keep them
        // as a fallback signal only when payload counters are unavailable.
        if let Some(bytes) = p.bytes_uploaded.or(p.net_bytes_uploaded) {
            let at = Instant::now();
            t.up_total_bytes = Some(bytes);
            t.up_rate.observe(at, bytes);
            t.up_bps = Some(t.up_rate.rate_at(at, bytes));
        }

        if let Some(bytes) = p.bytes_downloaded.or(p.net_bytes_downloaded) {
            let at = Instant::now();
            t.down_total_bytes = Some(bytes);
            t.down_rate.observe(at, bytes);
            t.down_bps = Some(t.down_rate.rate_at(at, bytes));
        }
    }

    fn mark_external_run_finish(
        &mut self,
        target_id: &str,
        task_id: &str,
        state: &str,
        error_code: Option<String>,
    ) {
        let Some(t) = self.targets.get_mut(target_id) else {
            return;
        };
        if t.external_task_id.as_deref() != Some(task_id) {
            return;
        }

        t.external_task_id = None;
        t.external_process_id = None;
        t.external_last_report_at = None;
        t.external_logging = None;
        t.active_task = None;
        let failed = state == "failed";
        t.state = if failed { "failed" } else { "idle" }.to_string();
        let duration_seconds = t
            .running_since
            .map(|started| now_unix_ms().saturating_sub(started) as f64 / 1000.0);
        let (bytes_uploaded, bytes_deduped) = t
            .progress
            .as_ref()
            .map(|progress| (progress.bytes_uploaded, progress.bytes_deduped))
            .unwrap_or((None, None));
        t.running_since = None;
        t.progress = None;
        t.up_bps = None;
        t.up_total_bytes = None;
        t.up_rate.reset(Instant::now(), 0);

        t.down_bps = None;
        t.down_total_bytes = None;
        t.down_rate.reset(Instant::now(), 0);
        t.last_run = Some(TargetRunSummary {
            finished_at: Some(
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
            duration_seconds,
            status: Some(if failed { "failed" } else { "succeeded" }.to_string()),
            error_code: failed.then(|| error_code.unwrap_or_else(|| "task.failed".to_string())),
            files_indexed: None,
            bytes_uploaded,
            bytes_deduped,
        });
    }

    fn on_progress(&mut self, target_id: &str, p: TaskProgress) {
        let Some(t) = self.targets.get_mut(target_id) else {
            return;
        };
        if t.state != "running" {
            t.state = "running".to_string();
        }
        if t.running_since.is_none() {
            t.running_since = Some(now_unix_ms());
        }
        t.progress = Some(Progress {
            phase: p.phase,
            files_total: p.files_total,
            files_done: p.files_done,
            source_files_total: p.source_files_total,
            source_bytes_total: p.source_bytes_total,
            source_bytes_need_upload_total: p.source_bytes_need_upload_total,
            chunks_total: p.chunks_total,
            chunks_done: p.chunks_done,
            bytes_read: p.bytes_read,
            upload_bytes_total: p.upload_bytes_total,
            bytes_uploaded_confirmed: p.bytes_uploaded_confirmed,
            bytes_uploaded_source: p.bytes_uploaded_source,
            bytes_uploaded: p.bytes_uploaded,
            bytes_downloaded: p.bytes_downloaded,
            bytes_deduped: p.bytes_deduped,
        });

        if let Some(bytes) = p.bytes_uploaded.or(p.net_bytes_uploaded) {
            t.up_total_bytes = Some(bytes);
            // Observe byte advances at the progress callback time to avoid attributing large
            // bursts to the much smaller status-tick cadence (which can cause brief spikes).
            t.up_rate.observe(Instant::now(), bytes);
        }

        if let Some(bytes) = p.bytes_downloaded.or(p.net_bytes_downloaded) {
            t.down_total_bytes = Some(bytes);
            t.down_rate.observe(Instant::now(), bytes);
        }
    }

    fn mark_run_finish_success(
        &mut self,
        target_id: &str,
        duration_seconds: f64,
        files_indexed: u64,
        bytes_uploaded: u64,
        bytes_deduped: u64,
    ) {
        let Some(t) = self.targets.get_mut(target_id) else {
            return;
        };
        if t.external_task_id.is_some()
            || t.active_task
                .as_ref()
                .is_none_or(|task| task.kind != "backup")
        {
            return;
        }
        t.state = "idle".to_string();
        t.active_task = None;
        t.running_since = None;
        t.progress = None;
        t.up_bps = None;
        t.up_total_bytes = None;
        t.up_rate = ByteRateWindow::default();
        t.down_bps = None;
        t.down_total_bytes = None;
        t.down_rate = ByteRateWindow::default();
        t.last_run = Some(TargetRunSummary {
            finished_at: Some(
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
            duration_seconds: Some(duration_seconds),
            status: Some("succeeded".to_string()),
            error_code: None,
            files_indexed: Some(files_indexed),
            bytes_uploaded: Some(bytes_uploaded),
            bytes_deduped: Some(bytes_deduped),
        });
    }

    fn mark_run_finish_failure(
        &mut self,
        target_id: &str,
        duration_seconds: f64,
        error_code: String,
    ) {
        let Some(t) = self.targets.get_mut(target_id) else {
            return;
        };
        if t.external_task_id.is_some()
            || t.active_task
                .as_ref()
                .is_none_or(|task| task.kind != "backup")
        {
            return;
        }
        t.state = "failed".to_string();
        t.active_task = None;
        t.running_since = None;
        t.progress = None;
        t.up_bps = None;
        t.up_total_bytes = None;
        t.up_rate = ByteRateWindow::default();
        t.down_bps = None;
        t.down_total_bytes = None;
        t.down_rate = ByteRateWindow::default();
        t.last_run = Some(TargetRunSummary {
            finished_at: Some(
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
            duration_seconds: Some(duration_seconds),
            status: Some("failed".to_string()),
            error_code: Some(error_code),
            files_indexed: None,
            bytes_uploaded: None,
            bytes_deduped: None,
        });
    }

    fn mark_run_finish_cancelled(&mut self, target_id: &str, duration_seconds: f64) {
        let Some(t) = self.targets.get_mut(target_id) else {
            return;
        };
        if t.external_task_id.is_some()
            || t.active_task
                .as_ref()
                .is_none_or(|task| task.kind != "backup")
        {
            return;
        }
        t.state = "idle".to_string();
        t.active_task = None;
        t.running_since = None;
        t.progress = None;
        t.up_bps = None;
        t.up_total_bytes = None;
        t.up_rate = ByteRateWindow::default();
        t.down_bps = None;
        t.down_total_bytes = None;
        t.down_rate = ByteRateWindow::default();
        t.last_run = Some(TargetRunSummary {
            finished_at: Some(
                chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            ),
            duration_seconds: Some(duration_seconds),
            status: Some("cancelled".to_string()),
            error_code: None,
            files_indexed: None,
            bytes_uploaded: None,
            bytes_deduped: None,
        });
    }

    fn has_running(&self) -> bool {
        self.targets.values().any(|t| t.state == "running")
    }

    fn target_is_busy(&self, target_id: &str) -> bool {
        self.targets
            .get(target_id)
            .is_some_and(|target| target.active_task.is_some() || target.state == "running")
    }

    fn reap_finished_external_tasks(&mut self, now: Instant) {
        self.reap_finished_external_tasks_at(now, is_process_alive);
    }

    fn reap_finished_external_tasks_at(
        &mut self,
        now: Instant,
        process_is_alive: impl Fn(u32) -> bool,
    ) {
        let reporters_lost: Vec<(String, String)> = self
            .targets
            .iter()
            .filter_map(|(target_id, target)| {
                let task_id = target.external_task_id.as_ref()?;
                let is_live = match target.external_process_id {
                    Some(pid) => process_is_alive(pid),
                    None => target.external_last_report_at.is_some_and(|last_report| {
                        now.saturating_duration_since(last_report) <= EXTERNAL_TASK_REPORT_TIMEOUT
                    }),
                };
                (!is_live).then(|| (target_id.clone(), task_id.clone()))
            })
            .collect();

        for (target_id, task_id) in reporters_lost {
            self.mark_external_run_finish(
                &target_id,
                &task_id,
                "failed",
                Some("task.reporter_lost".to_string()),
            );
        }
    }

    fn has_active_work(&self) -> bool {
        self.has_running()
            || self
                .targets
                .values()
                .any(|target| target.backup_queue.is_some())
    }

    fn set_backup_queue_memberships(
        &mut self,
        memberships: &HashMap<String, BackupQueueMembership>,
    ) {
        for (target_id, target) in &mut self.targets {
            target.backup_queue = memberships.get(target_id).cloned();
        }
    }

    fn active_external_logging(
        &self,
    ) -> Option<&televy_backup_core::local_settings::ResolvedLogging> {
        self.targets
            .values()
            .find(|target| target.external_task_id.is_some())
            .and_then(|target| target.external_logging.as_ref())
    }

    fn tick_rates_at(&mut self, now: Instant) {
        for t in self.targets.values_mut() {
            if t.state != "running" {
                continue;
            }
            if let Some(bytes) = t.up_total_bytes {
                if t.up_rate.is_empty() {
                    t.up_rate.reset(now, bytes);
                    t.up_bps = Some(0);
                } else {
                    t.up_bps = Some(t.up_rate.rate_at(now, bytes));
                }
            }

            if let Some(bytes) = t.down_total_bytes {
                if t.down_rate.is_empty() {
                    t.down_rate.reset(now, bytes);
                    t.down_bps = Some(0);
                } else {
                    t.down_bps = Some(t.down_rate.rate_at(now, bytes));
                }
            }
        }
    }

    fn build_snapshot(&self, now_ms: u64) -> StatusSnapshot {
        let mut global_up_bps: u64 = 0;
        let mut global_up_total: u64 = 0;
        let mut have_global_up = false;

        let mut global_down_bps: u64 = 0;
        let mut global_down_total: u64 = 0;
        let mut have_global_down = false;

        let mut out_targets = Vec::new();
        for id in &self.target_order {
            let Some(t) = self.targets.get(id) else {
                continue;
            };
            if let Some(bps) = t.up_bps {
                global_up_bps = global_up_bps.saturating_add(bps);
                have_global_up = true;
            }
            if let Some(bytes) = t.up_total_bytes {
                global_up_total = global_up_total.saturating_add(bytes);
                have_global_up = true;
            }

            if let Some(bps) = t.down_bps {
                global_down_bps = global_down_bps.saturating_add(bps);
                have_global_down = true;
            }
            if let Some(bytes) = t.down_total_bytes {
                global_down_total = global_down_total.saturating_add(bytes);
                have_global_down = true;
            }
            out_targets.push(TargetState {
                target_id: t.target_id.clone(),
                label: t.label.clone(),
                source_path: t.source_path.clone(),
                endpoint_id: t.endpoint_id.clone(),
                enabled: t.enabled,
                state: t.state.clone(),
                running_since: t.running_since,
                up: Rate {
                    bytes_per_second: t.up_bps,
                },
                up_total: Counter {
                    bytes: t.up_total_bytes,
                },
                progress: t.progress.clone(),
                last_run: t.last_run.clone(),
                active_task: t.active_task.clone(),
                backup_queue: t.backup_queue.clone(),
                extra: Default::default(),
            });
        }

        StatusSnapshot {
            type_: "status.snapshot".to_string(),
            schema_version: 1,
            generated_at: now_ms,
            source: StatusSource {
                kind: "daemon".to_string(),
                detail: Some("televybackupd (status.json)".to_string()),
            },
            global: GlobalStatus {
                up: Rate {
                    bytes_per_second: have_global_up.then_some(global_up_bps),
                },
                down: Rate {
                    bytes_per_second: have_global_down.then_some(global_down_bps),
                },
                up_total: Counter {
                    bytes: have_global_up.then_some(global_up_total),
                },
                down_total: Counter {
                    bytes: have_global_down.then_some(global_down_total),
                },
                ui_uptime_seconds: None,
            },
            targets: out_targets,
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone)]
struct ByteRateWindow {
    window: Duration,
    samples: VecDeque<(Instant, u64)>,
}

impl Default for ByteRateWindow {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(1),
            samples: VecDeque::new(),
        }
    }
}

impl ByteRateWindow {
    fn reset(&mut self, now: Instant, bytes: u64) {
        self.samples.clear();
        self.samples.push_back((now, bytes));
    }

    fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    fn observe(&mut self, at: Instant, bytes: u64) {
        if self
            .samples
            .back()
            .is_some_and(|(_, last_bytes)| bytes < *last_bytes)
        {
            self.reset(at, bytes);
            return;
        }

        // Only keep change points; this keeps the window small and prevents tick cadence from
        // introducing artificial dt shrinkage when bytes advance in large bursts.
        if self
            .samples
            .back()
            .is_some_and(|(_, last_bytes)| *last_bytes == bytes)
        {
            return;
        }

        self.samples.push_back((at, bytes));
    }

    fn rate_at(&mut self, now: Instant, bytes_now: u64) -> u64 {
        if self.is_empty() {
            self.reset(now, bytes_now);
            return 0;
        }

        if self
            .samples
            .back()
            .is_some_and(|(_, last_bytes)| bytes_now < *last_bytes)
        {
            self.reset(now, bytes_now);
            return 0;
        }

        // If the caller didn't observe the latest byte advance (e.g. missing progress callbacks),
        // record it at "now" as a best-effort fallback.
        self.observe(now, bytes_now);

        let cutoff = now.checked_sub(self.window).unwrap_or(now);

        // If the newest sample is older than the window, then no bytes advanced in the last window.
        //
        // Important: do NOT `reset(now, bytes_now)` here. When progress callbacks are coarse
        // (e.g. whole-part/object boundaries), the status writer keeps ticking even while bytes
        // are in-flight. If we reset the time base on every "no progress" window, the next byte
        // jump gets attributed to a tiny dt and shows up as an impossible spike.
        if self.samples.back().is_some_and(|(t, _)| *t <= cutoff) {
            return 0;
        }

        // Prune samples, but keep one sample older than the cutoff for interpolation.
        while self.samples.len() > 2 {
            let second_at = self.samples.get(1).map(|(t, _)| *t).unwrap_or(cutoff);
            if second_at <= cutoff {
                self.samples.pop_front();
            } else {
                break;
            }
        }

        let win_secs = self.window.as_secs_f64();
        if win_secs <= 0.0 {
            return 0;
        }

        // Estimate bytes at the cutoff time. We linearly interpolate between the last sample
        // before the cutoff and the first sample after it (when available).
        let bytes_at_cutoff = match (self.samples.front(), self.samples.get(1)) {
            (Some((t0, b0)), Some((t1, b1))) => {
                if *t0 >= cutoff {
                    *b0 as f64
                } else if *t1 <= cutoff {
                    // Shouldn't happen after pruning, but keep it safe.
                    *b1 as f64
                } else {
                    let total = t1.duration_since(*t0).as_secs_f64();
                    if total <= 0.0 {
                        *b1 as f64
                    } else {
                        let frac = cutoff.duration_since(*t0).as_secs_f64() / total;
                        (*b0 as f64) + ((*b1 as f64) - (*b0 as f64)) * frac
                    }
                }
            }
            (Some((_t0, b0)), None) => *b0 as f64,
            _ => bytes_now as f64,
        };

        let delta = (bytes_now as f64 - bytes_at_cutoff).max(0.0);
        (delta / win_secs).round() as u64
    }
}

#[derive(Clone)]
struct StatusProgressSink {
    target_id: String,
    state: Arc<Mutex<StatusRuntimeState>>,
}

impl ProgressSink for StatusProgressSink {
    fn on_progress(&self, progress: TaskProgress) {
        if let Ok(mut st) = self.state.lock() {
            st.on_progress(&self.target_id, progress);
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn daemon_instance_lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let _first = acquire_daemon_instance_lock(dir.path()).unwrap();
        let err = acquire_daemon_instance_lock(dir.path()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::AddrInUse);
    }

    #[test]
    fn backup_queue_coalesces_idle_requests_into_one_active_batch() {
        let mut queue = BackupQueue::default();
        let target_order = vec!["t1".to_string(), "t2".to_string(), "t3".to_string()];
        let (batch_id, disposition, targets) =
            queue.enqueue(vec!["t1".to_string(), "t2".to_string()], &target_order);
        assert_eq!(disposition, "accepted");
        assert_eq!(targets, vec!["t1", "t2"]);

        let (coalesced_id, disposition, targets) =
            queue.enqueue(vec!["t3".to_string(), "t2".to_string()], &target_order);
        assert_eq!(coalesced_id, batch_id);
        assert_eq!(disposition, "coalesced");
        assert_eq!(targets, vec!["t1", "t2", "t3"]);
    }

    #[test]
    fn backup_queue_runs_one_target_and_promotes_the_pending_batch() {
        let mut queue = BackupQueue::default();
        let target_order = vec!["t1".to_string(), "t2".to_string(), "t3".to_string()];
        let (active_id, _, _) =
            queue.enqueue(vec!["t1".to_string(), "t2".to_string()], &target_order);
        assert_eq!(queue.start_next_target().as_deref(), Some("t1"));

        let (pending_id, disposition, pending_targets) =
            queue.enqueue(vec!["t3".to_string()], &target_order);
        assert_eq!(disposition, "accepted");
        assert_ne!(pending_id, active_id);
        assert_eq!(pending_targets, vec!["t3"]);

        let (coalesced_id, disposition, pending_targets) =
            queue.enqueue(vec!["t2".to_string(), "t1".to_string()], &target_order);
        assert_eq!(disposition, "coalesced");
        assert_eq!(coalesced_id, pending_id);
        assert_eq!(pending_targets, vec!["t1", "t2", "t3"]);

        let memberships = queue.memberships();
        assert_eq!(
            memberships["t1"].active_batch_id.as_deref(),
            Some(active_id.as_str())
        );
        assert_eq!(
            memberships["t1"].pending_batch_id.as_deref(),
            Some(pending_id.as_str())
        );
        assert_eq!(
            memberships["t2"].active_batch_id.as_deref(),
            Some(active_id.as_str())
        );

        queue.complete_active_target("t1");
        assert_eq!(queue.start_next_target().as_deref(), Some("t2"));
        queue.complete_active_target("t2");
        assert_eq!(queue.start_next_target().as_deref(), Some("t1"));
        queue.complete_active_target("t1");
        assert_eq!(queue.start_next_target().as_deref(), Some("t2"));
        queue.complete_active_target("t2");
        assert_eq!(queue.start_next_target().as_deref(), Some("t3"));
        queue.complete_active_target("t3");
        assert!(!queue.has_work());
    }

    #[test]
    fn backup_queue_target_failure_records_the_failure_and_continues() {
        let queue = Arc::new(Mutex::new(BackupQueue::default()));
        let status = Arc::new(Mutex::new(state_one_target()));
        let target_order = vec!["t1".to_string(), "t2".to_string()];
        queue
            .lock()
            .unwrap()
            .enqueue(vec!["t1".to_string(), "t2".to_string()], &target_order);
        assert_eq!(
            queue.lock().unwrap().start_next_target().as_deref(),
            Some("t1")
        );
        status.lock().unwrap().mark_backup_run_start("t1");

        fail_backup_queue_target(&queue, &status, "t1", "target.not_found");

        let target = status.lock().unwrap().targets.get("t1").unwrap().clone();
        assert_eq!(target.state, "failed");
        assert_eq!(
            target
                .last_run
                .as_ref()
                .and_then(|run| run.error_code.as_deref()),
            Some("target.not_found")
        );
        assert_eq!(
            queue.lock().unwrap().start_next_target().as_deref(),
            Some("t2")
        );
    }

    #[test]
    fn backup_queue_memberships_apply_to_targets_added_during_settings_reload() {
        let mut old_settings = settings_config::SettingsV2::default();
        old_settings.targets.push(settings_config::Target {
            id: "existing".to_string(),
            source_path: "/tmp/existing".to_string(),
            label: "Existing".to_string(),
            endpoint_id: "ep1".to_string(),
            enabled: true,
            schedule: None,
        });
        let status = Arc::new(Mutex::new(StatusRuntimeState::from_settings(&old_settings)));
        let queue = Arc::new(Mutex::new(BackupQueue::default()));

        let target_order = vec!["existing".to_string(), "imported".to_string()];
        let batch_id = queue
            .lock()
            .unwrap()
            .enqueue(vec!["imported".to_string()], &target_order)
            .0;
        sync_backup_queue_memberships(&queue, &status);

        let mut reloaded_settings = old_settings;
        reloaded_settings.targets.push(settings_config::Target {
            id: "imported".to_string(),
            source_path: "/tmp/imported".to_string(),
            label: "Imported".to_string(),
            endpoint_id: "ep1".to_string(),
            enabled: true,
            schedule: None,
        });
        status.lock().unwrap().apply_settings(&reloaded_settings);
        sync_backup_queue_memberships(&queue, &status);

        let membership = status
            .lock()
            .unwrap()
            .targets
            .get("imported")
            .and_then(|target| target.backup_queue.as_ref())
            .cloned()
            .expect("reloaded target should retain its queue membership");
        assert_eq!(
            membership.active_batch_id.as_deref(),
            Some(batch_id.as_str())
        );
    }

    #[test]
    fn backup_queue_waits_for_requested_settings_reload_before_starting() {
        let queue = Arc::new(Mutex::new(BackupQueue::default()));
        let reload_requested = AtomicBool::new(true);
        queue
            .lock()
            .unwrap()
            .enqueue(vec!["imported".to_string()], &["imported".to_string()]);

        assert_eq!(
            start_next_queued_target(&queue, &reload_requested),
            None,
            "the queued target must wait until its settings are applied"
        );

        reload_requested.store(false, Ordering::Release);
        assert_eq!(
            start_next_queued_target(&queue, &reload_requested).as_deref(),
            Some("imported")
        );
    }

    #[test]
    fn queue_membership_projects_to_targets_added_while_another_target_runs() {
        let mut old_settings = settings_config::SettingsV2::default();
        old_settings.targets.push(settings_config::Target {
            id: "running".to_string(),
            source_path: "/tmp/running".to_string(),
            label: "Running".to_string(),
            endpoint_id: "ep1".to_string(),
            enabled: true,
            schedule: None,
        });
        let status = Arc::new(Mutex::new(StatusRuntimeState::from_settings(&old_settings)));
        status.lock().unwrap().mark_backup_run_start("running");
        let queue = Arc::new(Mutex::new(BackupQueue::default()));

        let mut imported_settings = old_settings;
        imported_settings.targets.push(settings_config::Target {
            id: "imported".to_string(),
            source_path: "/tmp/imported".to_string(),
            label: "Imported".to_string(),
            endpoint_id: "ep1".to_string(),
            enabled: true,
            schedule: None,
        });
        let batch_id = queue
            .lock()
            .unwrap()
            .enqueue(
                vec!["imported".to_string()],
                &["running".to_string(), "imported".to_string()],
            )
            .0;

        status
            .lock()
            .unwrap()
            .add_missing_targets(&imported_settings);
        sync_backup_queue_memberships(&queue, &status);

        let status = status.lock().unwrap();
        assert_eq!(status.targets["running"].state, "running");
        assert_eq!(
            status.targets["imported"]
                .backup_queue
                .as_ref()
                .and_then(|membership| membership.active_batch_id.as_deref()),
            Some(batch_id.as_str())
        );
    }

    fn state_one_target() -> StatusRuntimeState {
        let mut st = StatusRuntimeState {
            target_order: vec!["t1".to_string()],
            targets: HashMap::new(),
        };
        st.targets.insert(
            "t1".to_string(),
            TargetRuntime {
                target_id: "t1".to_string(),
                label: None,
                source_path: "/tmp".to_string(),
                endpoint_id: "ep".to_string(),
                enabled: true,
                state: "idle".to_string(),
                running_since: None,
                progress: None,
                last_run: None,
                active_task: None,
                backup_queue: None,
                external_task_id: None,
                external_process_id: None,
                external_last_report_at: None,
                external_logging: None,
                up_bps: None,
                up_total_bytes: None,
                up_rate: ByteRateWindow::default(),
                down_bps: None,
                down_total_bytes: None,
                down_rate: ByteRateWindow::default(),
            },
        );
        st
    }

    fn progress(bytes_uploaded: u64) -> TaskProgress {
        TaskProgress {
            phase: "upload".to_string(),
            files_total: None,
            files_done: None,
            source_files_total: None,
            source_bytes_total: None,
            source_bytes_need_upload_total: None,
            chunks_total: None,
            chunks_done: None,
            bytes_read: None,
            upload_bytes_total: None,
            bytes_uploaded_confirmed: None,
            bytes_uploaded_source: None,
            bytes_uploaded: Some(bytes_uploaded),
            net_bytes_uploaded: None,
            bytes_downloaded: None,
            net_bytes_downloaded: None,
            bytes_deduped: None,
        }
    }

    #[test]
    fn up_total_tracks_progress_bytes_uploaded() {
        let mut st = state_one_target();
        st.mark_run_start("t1");
        st.on_progress("t1", progress(123));
        assert_eq!(st.targets.get("t1").unwrap().up_total_bytes, Some(123));
    }

    #[test]
    fn external_task_activity_and_failure_are_live_only() {
        let mut st = state_one_target();
        st.mark_external_run_start("t1", "restore-1", "restore", None, None)
            .expect("restore should start");

        let active = st.targets["t1"].active_task.as_ref().expect("active task");
        assert_eq!(active.kind, "restore");
        assert_eq!(active.directions, vec!["down"]);

        assert_eq!(
            st.mark_external_run_start("t1", "verify-2", "verify", None, None)
                .expect_err("a second task on the same target must be rejected"),
            "restore"
        );

        st.mark_external_run_finish(
            "t1",
            "restore-1",
            "failed",
            Some("restore.network_failed".to_string()),
        );
        let target = &st.targets["t1"];
        assert!(target.active_task.is_none());
        assert_eq!(target.state, "failed");
        assert_eq!(
            target
                .last_run
                .as_ref()
                .and_then(|run| run.error_code.as_deref()),
            Some("restore.network_failed")
        );
    }

    #[test]
    fn external_tasks_can_run_on_different_targets() {
        let mut st = state_one_target();
        let mut second = st.targets["t1"].clone();
        second.target_id = "t2".to_string();
        st.target_order.push("t2".to_string());
        st.targets.insert("t2".to_string(), second);

        st.mark_external_run_start("t1", "restore-1", "restore", None, None)
            .expect("restore should start");
        st.mark_external_run_start("t2", "backup-1", "backup", None, None)
            .expect("backup should start on another target");

        let snapshot = st.build_snapshot(1_000);
        assert_eq!(
            snapshot.targets[0].active_task.as_ref().unwrap().kind,
            "restore"
        );
        assert_eq!(
            snapshot.targets[1].active_task.as_ref().unwrap().kind,
            "backup"
        );
    }

    #[test]
    fn daemon_backup_claim_excludes_external_tasks_on_the_same_target() {
        let mut st = state_one_target();
        assert!(st.try_mark_backup_run_start("t1", "connecting"));

        assert_eq!(
            st.mark_external_run_start("t1", "restore-1", "restore", None, None)
                .expect_err("an external task must not replace a daemon backup claim"),
            "backup"
        );
    }

    #[test]
    fn daemon_backup_completion_never_releases_an_external_task() {
        let mut st = state_one_target();
        st.mark_external_run_start("t1", "restore-1", "restore", None, None)
            .expect("restore should start");

        st.mark_run_finish_cancelled("t1", 0.0);
        st.mark_run_finish_failure("t1", 0.0, "backup.failed".to_string());
        st.mark_run_finish_success("t1", 0.0, 0, 0, 0);

        let target = &st.targets["t1"];
        assert_eq!(target.state, "running");
        assert_eq!(target.external_task_id.as_deref(), Some("restore-1"));
        assert_eq!(
            target.active_task.as_ref().map(|task| task.kind.as_str()),
            Some("restore")
        );
    }

    #[test]
    fn missing_external_reporter_releases_target() {
        let mut st = state_one_target();
        st.mark_external_run_start("t1", "restore-1", "restore", Some(42), None)
            .expect("restore should start");

        st.reap_finished_external_tasks_at(Instant::now(), |_| false);

        let target = &st.targets["t1"];
        assert!(!st.target_is_busy("t1"));
        assert_eq!(target.state, "failed");
        assert!(target.active_task.is_none());
        assert_eq!(
            target
                .last_run
                .as_ref()
                .and_then(|run| run.error_code.as_deref()),
            Some("task.reporter_lost")
        );
    }

    #[test]
    fn snapshots_publish_explicit_backup_activity() {
        let mut st = state_one_target();
        st.mark_backup_run_start("t1");

        let snapshot = st.build_snapshot(1_000);
        let active = snapshot.targets[0]
            .active_task
            .as_ref()
            .expect("backup activity");
        assert_eq!(active.kind, "backup");
        assert_eq!(active.directions, vec!["up"]);
    }

    #[test]
    fn byte_rate_window_uses_rolling_1s_window() {
        let t0 = Instant::now();
        let mut w = ByteRateWindow::default();

        w.reset(t0, 0);
        assert_eq!(w.rate_at(t0, 0), 0);

        // With a fixed 1s window, the early rate is averaged over 1 second to avoid spikes.
        w.observe(t0 + Duration::from_millis(500), 1000);
        assert_eq!(w.rate_at(t0 + Duration::from_millis(500), 1000), 1000);

        // Another 1000 bytes over the next 1.0s => 1000 B/s steady state.
        w.observe(t0 + Duration::from_millis(1500), 2000);
        assert_eq!(w.rate_at(t0 + Duration::from_millis(1500), 2000), 1000);
    }

    #[test]
    fn byte_rate_window_interpolates_to_avoid_burst_spikes() {
        let t0 = Instant::now();
        let mut w = ByteRateWindow::default();

        w.reset(t0, 0);

        // A large byte jump after 5s should not turn into an absurd one-tick spike.
        w.observe(t0 + Duration::from_secs(5), 10_000);

        // Over a 1s window at t=5s, interpolation estimates ~2000 B/s (10_000 / 5s).
        assert_eq!(w.rate_at(t0 + Duration::from_secs(5), 10_000), 2000);
    }

    #[test]
    fn byte_rate_window_does_not_spike_after_idle_ticks() {
        let t0 = Instant::now();
        let mut w = ByteRateWindow::default();

        w.reset(t0, 0);

        // Simulate the status writer ticking frequently while bytes don't change.
        // Historically, resetting the window on every "idle" tick caused the next byte jump to
        // be attributed to a tiny dt and show up as an impossible one-tick spike.
        for i in 1..=50 {
            let now = t0 + Duration::from_millis(200 * i);
            assert_eq!(w.rate_at(now, 0), 0);
        }

        // A big jump after 10s should be interpreted as a ~1 KB/s steady transfer, not 10 KB/s.
        w.observe(t0 + Duration::from_secs(10), 10_000);
        assert_eq!(w.rate_at(t0 + Duration::from_secs(10), 10_000), 1000);
    }

    #[test]
    fn up_bps_drops_to_zero_after_bytes_stall() {
        let mut st = state_one_target();
        st.mark_run_start("t1");

        let t0 = Instant::now();
        {
            let t = st.targets.get_mut("t1").unwrap();
            t.up_total_bytes = Some(0);
            t.up_rate.reset(t0, 0);
        }

        // First: bytes increase and we observe a non-zero rate.
        st.on_progress("t1", progress(1024));
        st.tick_rates_at(t0 + Duration::from_millis(200));
        assert!(st.targets.get("t1").unwrap().up_bps.unwrap_or(0) > 0);

        // Then: bytes do not change for > 1s; rate should fall to 0 even without progress events.
        st.tick_rates_at(t0 + Duration::from_millis(1500));
        assert_eq!(st.targets.get("t1").unwrap().up_bps, Some(0));
    }
}

async fn status_writer_loop(state: Arc<Mutex<StatusRuntimeState>>, status_path: PathBuf) {
    let mut last_write = Instant::now()
        .checked_sub(Duration::from_secs(3600))
        .unwrap_or_else(Instant::now);

    loop {
        let now = Instant::now();
        let has_active_work = state
            .lock()
            .ok()
            .map(|mut st| {
                st.reap_finished_external_tasks(now);
                st.has_active_work()
            })
            .unwrap_or(false);

        let min_interval = if has_active_work {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(1)
        };
        let should_write = now.duration_since(last_write) >= min_interval;
        if should_write {
            let snapshot_opt = {
                match state.lock() {
                    Ok(mut st) => {
                        st.reap_finished_external_tasks(now);
                        st.tick_rates_at(now);
                        Some(st.build_snapshot(now_unix_ms()))
                    }
                    Err(_) => None,
                }
            };
            let snapshot = match snapshot_opt {
                Some(s) => s,
                None => {
                    sleep(Duration::from_millis(100)).await;
                    continue;
                }
            };

            // Writing status snapshots is sync I/O + fsync-heavy; keep it off Tokio worker threads.
            // Status snapshots are "best-effort" and do not need durability guarantees; atomic rename is sufficient.
            let options = StatusWriteOptions {
                fsync_file: false,
                fsync_dir: false,
            };
            let status_path_for_write = status_path.clone();
            let status_path_for_log = status_path.clone();
            let res = tokio::task::spawn_blocking(move || {
                write_status_snapshot_json_atomic_with_options(
                    &status_path_for_write,
                    &snapshot,
                    options,
                )
            })
            .await;
            match res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(
                        event = "status.write_failed",
                        error = %e,
                        path = %status_path_for_log.display(),
                        "status.write_failed"
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        event = "status.write_failed",
                        error = %e,
                        path = %status_path_for_log.display(),
                        "status.write_failed"
                    );
                }
            }
            last_write = Instant::now();
        }

        let tick = if has_active_work {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(200)
        };
        sleep(tick).await;
    }
}

fn file_mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
}

#[derive(Debug, Clone)]
struct VaultKeyLoadError {
    code: Option<i32>,
    message: String,
}

impl VaultKeyLoadError {
    fn is_keychain_error(&self) -> bool {
        self.code.is_some()
    }
}

fn vault_key_error_code(_err: &(dyn std::error::Error + 'static)) -> Option<i32> {
    #[cfg(target_os = "macos")]
    {
        if let Some(e) = _err.downcast_ref::<security_framework::base::Error>() {
            return Some(e.code());
        }
    }
    None
}

fn vault_key_load_error(err: &(dyn std::error::Error + 'static)) -> VaultKeyLoadError {
    VaultKeyLoadError {
        code: vault_key_error_code(err),
        message: err.to_string(),
    }
}

#[cfg(unix)]
fn acquire_daemon_instance_lock(data_root: &Path) -> std::io::Result<File> {
    let lock_path = data_root.join("ipc").join("daemon.lock");
    if let Some(parent) = lock_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)?;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        let err = std::io::Error::last_os_error();
        let raw = err.raw_os_error().unwrap_or_default();
        if raw == libc::EWOULDBLOCK || raw == libc::EAGAIN {
            return Err(std::io::Error::new(
                ErrorKind::AddrInUse,
                format!(
                    "televybackupd already running for data dir {}; lock={}",
                    data_root.display(),
                    lock_path.display()
                ),
            ));
        }
        return Err(err);
    }

    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    writeln!(file, "{}", std::process::id())?;
    file.flush()?;

    Ok(file)
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let config_dir = std::env::var("TELEVYBACKUP_CONFIG_DIR")
        .ok()
        .map(PathBuf::from);
    let data_dir = std::env::var("TELEVYBACKUP_DATA_DIR")
        .ok()
        .map(PathBuf::from);

    let config_root = config_dir.unwrap_or_else(default_config_dir);
    let data_root = data_dir.unwrap_or_else(default_data_dir);
    let index_dir = data_root.join("index");
    #[cfg(unix)]
    let _daemon_instance_lock = acquire_daemon_instance_lock(&data_root)?;

    let config_path = settings_config::config_path(&config_root);
    let mut settings = settings_config::load_settings_v2(&config_root)?;
    let _ = CONFIG_ROOT_CACHE.set(config_root.clone());
    settings_config::validate_settings_schema_v2(&settings)?;
    let mut last_config_mtime = file_mtime(&config_path);

    // The development vault backend creates its first master key automatically. Do that before
    // control IPC becomes reachable so a first backup request cannot observe an empty store.
    initialize_dev_master_key_if_missing(&config_root)?;

    let status_state = Arc::new(Mutex::new(StatusRuntimeState::from_settings(&settings)));
    let backup_queue = Arc::new(Mutex::new(BackupQueue::default()));
    let backup_queue_notify = Arc::new(Notify::new());
    let settings_reload_requested = Arc::new(AtomicBool::new(false));
    let lifecycle = Arc::new(DaemonLifecycle::default());
    let runtime_logging = Arc::new(RwLock::new(televy_backup_core::local_settings::resolve(
        &config_root,
    )));
    let status_path = status_json_path(&data_root);
    tokio::spawn(status_writer_loop(status_state.clone(), status_path));

    let ipc_socket_path = status_ipc_socket_path(&data_root);
    let ipc_state = status_state.clone();
    let _status_ipc_server = match status_ipc::spawn_status_ipc_server(
        ipc_socket_path.clone(),
        Arc::new(move || {
            let now_ms = now_unix_ms();
            match ipc_state.lock() {
                Ok(mut st) => {
                    st.reap_finished_external_tasks(Instant::now());
                    let has_running = st.has_active_work();
                    // The GUI primarily reads status via IPC; keep rate sampling ticking even if
                    // progress callbacks pause so the UI doesn't get stuck on stale rates.
                    st.tick_rates_at(Instant::now());
                    let mut snap = st.build_snapshot(now_ms);
                    snap.source.detail = Some("televybackupd (ipc)".to_string());
                    (snap, has_running)
                }
                Err(_) => {
                    let snap = StatusSnapshot {
                        type_: "status.snapshot".to_string(),
                        schema_version: 1,
                        generated_at: now_ms,
                        source: StatusSource {
                            kind: "daemon".to_string(),
                            detail: Some("televybackupd (ipc)".to_string()),
                        },
                        global: GlobalStatus {
                            up: Rate {
                                bytes_per_second: None,
                            },
                            down: Rate {
                                bytes_per_second: None,
                            },
                            up_total: Counter { bytes: None },
                            down_total: Counter { bytes: None },
                            ui_uptime_seconds: None,
                        },
                        targets: Vec::new(),
                        extra: Default::default(),
                    };
                    (snap, false)
                }
            }
        }),
    ) {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!(
                "WARN: status.ipc_bind_failed: path={} error={}",
                ipc_socket_path.display(),
                e
            );
            tracing::warn!(
                event = "status.ipc_bind_failed",
                error = %e,
                path = %ipc_socket_path.display(),
                "status.ipc_bind_failed"
            );
            None
        }
    };

    let vault_socket_path = televy_backup_core::secrets::vault_ipc_socket_path(&data_root);
    let _vault_ipc_server = match vault_ipc::spawn_vault_ipc_server(vault_socket_path.clone()) {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!(
                "WARN: vault.ipc_bind_failed: path={} error={}",
                vault_socket_path.display(),
                e
            );
            tracing::warn!(
                event = "vault.ipc_bind_failed",
                error = %e,
                path = %vault_socket_path.display(),
                "vault.ipc_bind_failed"
            );
            None
        }
    };

    let control_ipc_settings = Arc::new(RwLock::new(settings.clone()));

    let control_socket_path = televy_backup_core::control::control_ipc_socket_path(&data_root);
    let _control_ipc_server = match control_ipc::spawn_control_ipc_server(
        control_socket_path.clone(),
        control_ipc::ControlContext {
            config_root: config_root.clone(),
            settings: control_ipc_settings.clone(),
            status_state: status_state.clone(),
            backup_queue: backup_queue.clone(),
            backup_queue_notify: backup_queue_notify.clone(),
            settings_reload_requested: settings_reload_requested.clone(),
            lifecycle: lifecycle.clone(),
            runtime_logging: runtime_logging.clone(),
            data_root: data_root.clone(),
        },
    ) {
        Ok(h) => Some(h),
        Err(e) => {
            eprintln!(
                "WARN: control.ipc_bind_failed: path={} error={}",
                control_socket_path.display(),
                e
            );
            tracing::warn!(
                event = "control.ipc_bind_failed",
                error = %e,
                path = %control_socket_path.display(),
                "control.ipc_bind_failed"
            );
            None
        }
    };

    let mut has_enabled_targets = settings.targets.iter().any(|t| t.enabled);
    if has_enabled_targets {
        if settings.telegram.mtproto.api_id <= 0 {
            return Err("telegram.mtproto.api_id must be > 0".into());
        }
        if settings.telegram.mtproto.api_hash_key.trim().is_empty() {
            return Err("telegram.mtproto.api_hash_key must not be empty".into());
        }
    }
    let secrets_path = televy_backup_core::secrets::secrets_path(&config_root);
    let mut secrets_file_exists = secrets_path.exists();
    let mut last_secrets_mtime = file_mtime(&secrets_path);
    let mut last_secrets_crypto_error_mtime: Option<SystemTime> = None;

    // Vault key (Keychain on macOS) can block on user auth/permission. Load it in the background
    // so the daemon can still serve status and control IPC while waiting.
    let mut vault_key: Option<[u8; 32]> = get_cached_vault_key();
    let mut vault_key_loader: Option<tokio::task::JoinHandle<Result<[u8; 32], VaultKeyLoadError>>> =
        None;
    let mut vault_key_last_attempt: Option<Instant> = None;
    let mut vault_key_last_error: Option<VaultKeyLoadError> = None;

    let mut secrets_store: Option<televy_backup_core::secrets::SecretsStore> = None;
    let mut master_key: Option<[u8; 32]> = None;
    let mut api_hash: Option<String> = None;

    // Do not eagerly retry Keychain access. We'll do a single best-effort warm-up attempt so
    // scheduled runs can work when Keychain is already unlocked, but avoid pestering the user
    // with repeated authorization prompts when they cancel.
    if vault_key.is_none() {
        vault_key_last_attempt = Some(Instant::now());
        vault_key_loader = Some(tokio::task::spawn_blocking(|| {
            load_or_create_vault_key_uncached().map_err(|e| vault_key_load_error(e.as_ref()))
        }));
    }

    let mut schedule_state_by_target = HashMap::<String, TargetScheduleState>::new();
    let mut storage_by_endpoint = HashMap::<String, TelegramMtProtoStorage>::new();

    loop {
        if lifecycle.is_shutdown_requested() {
            break;
        }
        let now = chrono::Local::now();

        // Hot-reload settings + secrets when files change. This avoids confusing situations where the
        // UI saved new endpoint chat_id but the long-running daemon kept using the old one.
        let has_running = status_state.lock().ok().is_some_and(|st| st.has_running());
        if !has_running {
            let next_logging = televy_backup_core::local_settings::resolve(&config_root);
            if *runtime_logging.read().await != next_logging {
                *runtime_logging.write().await = next_logging;
            }
            let config_mtime = file_mtime(&config_path);
            let secrets_mtime = file_mtime(&secrets_path);
            let config_changed = settings_reload_requested.swap(false, Ordering::AcqRel)
                || (config_mtime.is_some() && config_mtime != last_config_mtime);
            let secrets_changed = secrets_mtime.is_some() && secrets_mtime != last_secrets_mtime;

            if config_changed || secrets_changed {
                if config_changed {
                    match settings_config::load_settings_v2(&config_root) {
                        Ok(new_settings) => {
                            if let Err(e) =
                                settings_config::validate_settings_schema_v2(&new_settings)
                            {
                                tracing::warn!(
                                    event = "config.reload_failed",
                                    error = %e,
                                    path = %config_path.display(),
                                    "config.reload_failed"
                                );
                            } else {
                                settings = new_settings;
                                has_enabled_targets = settings.targets.iter().any(|t| t.enabled);
                                *control_ipc_settings.write().await = settings.clone();
                                last_config_mtime = config_mtime;
                                clear_mtproto_storage_cache(
                                    &mut storage_by_endpoint,
                                    "config_reloaded",
                                )
                                .await;
                                schedule_state_by_target
                                    .retain(|k, _| settings.targets.iter().any(|t| t.id == *k));
                                if let Ok(mut st) = status_state.lock() {
                                    st.apply_settings(&settings);
                                }
                                sync_backup_queue_memberships(&backup_queue, &status_state);
                                tracing::info!(
                                    event = "config.reloaded",
                                    path = %config_path.display(),
                                    "config.reloaded"
                                );
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                event = "config.reload_failed",
                                error = %e,
                                path = %config_path.display(),
                                "config.reload_failed"
                            );
                        }
                    }
                }

                if secrets_changed {
                    // Reload once the vault key is available. Until then, remember the mtime so we
                    // don't miss the change.
                    last_secrets_mtime = secrets_mtime;
                    secrets_file_exists = secrets_path.exists();
                    secrets_store = None;
                    master_key = None;
                    api_hash = None;
                    last_secrets_crypto_error_mtime = None;
                    clear_mtproto_storage_cache(&mut storage_by_endpoint, "secrets_changed").await;

                    tracing::info!(
                        event = "secrets.changed",
                        path = %secrets_path.display(),
                        "secrets.changed"
                    );
                }
            }
        }

        // Poll vault key loader (Keychain can block; never block the main loop).
        //
        // IMPORTANT: the vault IPC server may populate `VAULT_KEY_CACHE` independently; refresh
        // our local view so a successful `televybackup vault ensure` immediately unblocks runs.
        if vault_key.is_none()
            && let Some(k) = get_cached_vault_key()
        {
            vault_key = Some(k);
            vault_key_last_error = None;
            if let Some(h) = vault_key_loader.take() {
                h.abort();
            }
        }

        if vault_key.is_none() {
            let last_keychain_error = vault_key_last_error
                .as_ref()
                .is_some_and(|e| e.is_keychain_error());

            let should_retry = vault_key_loader.is_none()
                && vault_key_last_attempt
                    .map(|t| t.elapsed() >= Duration::from_secs(5))
                    .unwrap_or(true)
                && !last_keychain_error;
            if should_retry {
                vault_key_last_attempt = Some(Instant::now());
                vault_key_last_error = None;
                vault_key_loader = Some(tokio::task::spawn_blocking(|| {
                    load_or_create_vault_key_uncached()
                        .map_err(|e| vault_key_load_error(e.as_ref()))
                }));
            }

            if vault_key_loader.as_ref().is_some_and(|t| t.is_finished()) {
                match vault_key_loader.take().unwrap().await {
                    Ok(Ok(key)) => {
                        vault_key = Some(key);
                        set_cached_vault_key(key);
                        vault_key_last_error = None;
                    }
                    Ok(Err(e)) => {
                        vault_key_last_error = Some(e);
                    }
                    Err(e) => {
                        vault_key_last_error = Some(VaultKeyLoadError {
                            code: None,
                            message: e.to_string(),
                        });
                    }
                }
            }
        }

        // (Re)load secrets store and derived secrets once the vault key is ready.
        if secrets_store.is_none()
            && let Some(vault_key_bytes) = vault_key
        {
            match televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key_bytes) {
                Ok(store) => {
                    secrets_store = Some(store);
                    last_secrets_crypto_error_mtime = None;
                }
                Err(televy_backup_core::secrets::SecretsStoreError::Crypto) => {
                    // `secrets.enc` may have been replaced/rotated while the daemon was running.
                    // If we keep serving a stale cached vault key, the CLI/UI will be stuck with
                    // `crypto error` until restart. Invalidate cache once per secrets mtime and
                    // allow the existing Keychain retry guardrails to apply.
                    if last_secrets_mtime.is_some()
                        && last_secrets_mtime != last_secrets_crypto_error_mtime
                    {
                        tracing::warn!(
                            event = "secrets.crypto_failed",
                            path = %secrets_path.display(),
                            "secrets.crypto_failed (clearing cached vault key)"
                        );
                        last_secrets_crypto_error_mtime = last_secrets_mtime;
                        clear_cached_vault_key();
                        vault_key = None;
                        vault_key_last_error = None;
                        vault_key_last_attempt = None;
                        if let Some(h) = vault_key_loader.take() {
                            h.abort();
                        }
                    } else {
                        tracing::warn!(
                            event = "secrets.crypto_failed",
                            path = %secrets_path.display(),
                            "secrets.crypto_failed"
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        event = "secrets.reload_failed",
                        error = %e,
                        path = %secrets_path.display(),
                        "secrets.reload_failed"
                    );
                }
            }
        }

        if let Some(store) = secrets_store.as_mut() {
            // Master key is required for all backup/restore operations. In dev mode (keychain disabled),
            // auto-generate a master key on first run if no secrets file exists yet.
            if master_key.is_none() {
                let v = get_secret_from_store(store, MASTER_KEY_KEY);
                match v {
                    Some(b64) => {
                        if let Ok(k) = decode_base64_32(&b64) {
                            master_key = Some(k);
                        }
                    }
                    None if keychain_disabled() && !secrets_file_exists => {
                        if let Some(vault_key) = vault_key {
                            let mut bytes = [0u8; 32];
                            getrandom::getrandom(&mut bytes).map_err(|e| {
                                std::io::Error::other(format!("getrandom failed: {e}"))
                            })?;
                            let b64 = televy_backup_core::secrets::vault_key_to_base64(&bytes);
                            store.set(MASTER_KEY_KEY, b64.clone());
                            televy_backup_core::secrets::save_secrets_store(
                                &secrets_path,
                                &vault_key,
                                store,
                            )?;
                            secrets_file_exists = true;
                            master_key = Some(bytes);
                        }
                    }
                    None => {}
                }
            }

            if api_hash.is_none() {
                api_hash = get_secret_from_store(store, &settings.telegram.mtproto.api_hash_key);
            }
        }

        let has_queued_batch = backup_queue
            .lock()
            .map(|queue| queue.has_work())
            .unwrap_or(false);
        let needs_credentials = has_enabled_targets || has_queued_batch;
        let can_attempt_run = needs_credentials
            && vault_key.is_some()
            && secrets_store.is_some()
            && master_key.is_some()
            && api_hash.is_some();

        if settings.telegram.mtproto.api_id <= 0
            || settings.telegram.mtproto.api_hash_key.trim().is_empty()
        {
            // Keep the daemon alive so the UI can show status, but skip running backups until config is fixed.
            clear_mtproto_storage_cache(&mut storage_by_endpoint, "invalid_mtproto_api_config")
                .await;
            sleep(Duration::from_secs(1)).await;
            continue;
        }

        if needs_credentials && !can_attempt_run {
            let code = vault_key_last_error
                .as_ref()
                .map(|e| {
                    e.code
                        .map(|c| format!("{c}"))
                        .unwrap_or_else(|| e.message.clone())
                })
                .unwrap_or_else(|| "pending".to_string());
            tracing::warn!(
                event = "run.skip",
                kind = "backup",
                reason = "secrets_unavailable",
                detail = %code,
                "run.skip"
            );
            clear_mtproto_storage_cache(&mut storage_by_endpoint, "secrets_unavailable").await;
            sleep(Duration::from_secs(1)).await;
            continue;
        }

        if !needs_credentials {
            let shutdown_token = lifecycle.shutdown_token();
            tokio::select! {
                _ = sleep(Duration::from_secs(1)) => {}
                _ = backup_queue_notify.notified() => {}
                _ = shutdown_token.cancelled() => break,
            }
            continue;
        }

        std::fs::create_dir_all(&index_dir)?;

        let vault_key = vault_key.expect("vault key must be available when starting runs");
        let master_key = master_key.expect("master key must be available when starting runs");
        let api_hash = api_hash
            .clone()
            .expect("api_hash must be available when starting runs");

        let queued_target_id =
            start_next_queued_target(&backup_queue, settings_reload_requested.as_ref());
        if queued_target_id.is_none() && settings_reload_requested.load(Ordering::Acquire) {
            continue;
        }
        if let Some(target_id) = queued_target_id.as_deref()
            && !settings.targets.iter().any(|target| target.id == target_id)
        {
            let task_id = format!("tsk_{}", Uuid::new_v4());
            let logging = televy_backup_core::local_settings::resolve(&config_root);
            let run_log = televy_backup_core::run_log::start_run_log_with_retention(
                "backup",
                &task_id,
                &data_root,
                &logging.effective_filter,
                logging.retention_prune_enabled.then_some(logging.retention),
            )?;
            tracing::warn!(
                event = "run.start",
                kind = "backup",
                run_id = %task_id,
                task_id = %task_id,
                target_id,
                "run.start"
            );
            tracing::error!(
                event = "run.finish",
                kind = "backup",
                run_id = %task_id,
                task_id = %task_id,
                status = "failed",
                error_code = "target.not_found",
                target_id,
                "queued target no longer exists"
            );
            drop(run_log);
            fail_backup_queue_target(&backup_queue, &status_state, target_id, "target.not_found");
            continue;
        }

        for target in &settings.targets {
            let is_queued_target = queued_target_id.as_deref() == Some(target.id.as_str());
            if queued_target_id.is_some() && !is_queued_target {
                continue;
            }
            if queued_target_id.is_none() && !target.enabled {
                continue;
            }

            let state = schedule_state_by_target
                .entry(target.id.clone())
                .or_default();

            let scheduled_slot = if is_queued_target {
                Some(ScheduleSlot::Manual)
            } else {
                let eff = settings_config::effective_schedule(
                    &settings.schedule,
                    target.schedule.as_ref(),
                );
                if !eff.enabled {
                    None
                } else {
                    match eff.kind.as_str() {
                        "hourly" => {
                            if now.minute() != eff.hourly_minute as u32 {
                                None
                            } else {
                                let key = (now.year(), now.month(), now.day(), now.hour());
                                if state.last_hourly == Some(key) {
                                    None
                                } else {
                                    Some(ScheduleSlot::Hourly(key))
                                }
                            }
                        }
                        "daily" => {
                            let (hh, mm) = parse_hhmm(&eff.daily_at)?;
                            if now.hour() != hh as u32 || now.minute() != mm as u32 {
                                None
                            } else {
                                let key = (now.year(), now.month(), now.day());
                                if state.last_daily == Some(key) {
                                    None
                                } else {
                                    Some(ScheduleSlot::Daily(key))
                                }
                            }
                        }
                        other => {
                            return Err(format!("unsupported schedule.kind: {other}").into());
                        }
                    }
                }
            };

            let Some(scheduled_slot) = scheduled_slot else {
                continue;
            };

            let claimed = status_state
                .lock()
                .map(|mut status| status.try_mark_backup_run_start(&target.id, "connecting"))
                .unwrap_or(false);
            if !claimed {
                if is_queued_target {
                    tracing::info!(
                        event = "backup.queue_target_delayed",
                        target_id = %target.id,
                        reason = "target_busy",
                        "backup.queue_target_delayed"
                    );
                    sleep(Duration::from_secs(1)).await;
                } else {
                    tracing::info!(
                        event = "backup.scheduled_target_delayed",
                        target_id = %target.id,
                        reason = "target_busy",
                        "backup.scheduled_target_delayed"
                    );
                }
                continue;
            }

            let task_id = format!("tsk_{}", Uuid::new_v4());
            let logging = televy_backup_core::local_settings::resolve(&config_root);
            if *runtime_logging.read().await != logging {
                *runtime_logging.write().await = logging.clone();
            }
            let run_log = televy_backup_core::run_log::start_run_log_with_retention(
                "backup",
                &task_id,
                &data_root,
                &logging.effective_filter,
                logging.retention_prune_enabled.then_some(logging.retention),
            )?;
            let started = Instant::now();

            // Record the run before endpoint resolution and Telegram connection so the
            // externally visible connecting phase has a matching task/run identity.
            tracing::warn!(
                event = "run.start",
                kind = "backup",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %target.id,
                endpoint_id = %target.endpoint_id,
                source_path = %target.source_path,
                log_path = %run_log.path().display(),
                "run.start"
            );

            let task_cancel = if is_queued_target {
                let Ok(queue) = backup_queue.lock() else {
                    backup_task_cancelled(&status_state, &target.id, 0.0);
                    continue;
                };
                if !queue.active_target_matches(&target.id) {
                    tracing::warn!(
                        event = "run.finish",
                        kind = "backup",
                        run_id = %task_id,
                        task_id = %task_id,
                        status = "cancelled",
                        target_id = %target.id,
                        "run.finish"
                    );
                    backup_task_cancelled(&status_state, &target.id, 0.0);
                    continue;
                }
                lifecycle.begin_task()
            } else {
                lifecycle.begin_task()
            };

            if task_cancel.is_cancelled() {
                tracing::warn!(
                    event = "run.finish",
                    kind = "backup",
                    run_id = %task_id,
                    task_id = %task_id,
                    status = "cancelled",
                    target_id = %target.id,
                    "run.finish"
                );
                backup_task_cancelled(&status_state, &target.id, 0.0);
                lifecycle.finish_task();
                if is_queued_target {
                    complete_backup_queue_target(&backup_queue, &status_state, &target.id);
                }
                continue;
            }

            let Some(ep) = settings
                .telegram_endpoints
                .iter()
                .find(|e| e.id == target.endpoint_id)
            else {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    run_id = %task_id,
                    task_id = %task_id,
                    status = "failed",
                    error_code = "config.invalid",
                    error_message = "target references unknown endpoint_id",
                    target_id = %target.id,
                    endpoint_id = %target.endpoint_id,
                    "run.finish"
                );
                backup_task_failed(
                    &status_state,
                    &target.id,
                    started.elapsed().as_secs_f64(),
                    "config.invalid",
                );
                if is_queued_target {
                    complete_backup_queue_target(&backup_queue, &status_state, &target.id);
                }
                lifecycle.finish_task();
                continue;
            };

            if ep.chat_id.trim().is_empty() {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    run_id = %task_id,
                    task_id = %task_id,
                    status = "failed",
                    error_code = "config.invalid",
                    error_message = "endpoint chat_id is empty",
                    target_id = %target.id,
                    endpoint_id = %ep.id,
                    "run.finish"
                );
                backup_task_failed(
                    &status_state,
                    &target.id,
                    started.elapsed().as_secs_f64(),
                    "config.invalid",
                );
                if is_queued_target {
                    complete_backup_queue_target(&backup_queue, &status_state, &target.id);
                }
                lifecycle.finish_task();
                continue;
            }

            let bot_token = secrets_store
                .as_ref()
                .and_then(|s| get_secret_from_store(s, &ep.bot_token_key));
            let Some(bot_token) = bot_token else {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    run_id = %task_id,
                    task_id = %task_id,
                    status = "failed",
                    error_code = "telegram.unauthorized",
                    error_message = "bot token missing",
                    target_id = %target.id,
                    endpoint_id = %ep.id,
                    "run.finish"
                );
                backup_task_failed(
                    &status_state,
                    &target.id,
                    started.elapsed().as_secs_f64(),
                    "telegram.unauthorized",
                );
                if is_queued_target {
                    complete_backup_queue_target(&backup_queue, &status_state, &target.id);
                }
                lifecycle.finish_task();
                continue;
            };

            if !storage_by_endpoint.contains_key(&ep.id) {
                let session = match secrets_store
                    .as_ref()
                    .and_then(|s| get_secret_from_store(s, &ep.mtproto.session_key))
                {
                    Some(b64) if !b64.trim().is_empty() => {
                        match base64::engine::general_purpose::STANDARD.decode(b64.as_bytes()) {
                            Ok(session) => Some(session),
                            Err(error) => {
                                tracing::error!(
                                    event = "run.finish",
                                    kind = "backup",
                                    run_id = %task_id,
                                    task_id = %task_id,
                                    status = "failed",
                                    error_code = "telegram.mtproto.session_invalid",
                                    error_message = %error,
                                    target_id = %target.id,
                                    "run.finish"
                                );
                                backup_task_failed(
                                    &status_state,
                                    &target.id,
                                    started.elapsed().as_secs_f64(),
                                    "telegram.mtproto.session_invalid",
                                );
                                if is_queued_target {
                                    complete_backup_queue_target(
                                        &backup_queue,
                                        &status_state,
                                        &target.id,
                                    );
                                }
                                lifecycle.finish_task();
                                continue;
                            }
                        }
                    }
                    _ => None,
                };

                let cache_dir = data_root.join("cache").join("mtproto").join(&ep.id);
                if let Err(error) = std::fs::create_dir_all(&cache_dir) {
                    tracing::error!(
                        event = "run.finish",
                        kind = "backup",
                        run_id = %task_id,
                        task_id = %task_id,
                        status = "failed",
                        error_code = "config.write_failed",
                        error_message = %error,
                        target_id = %target.id,
                        "run.finish"
                    );
                    backup_task_failed(
                        &status_state,
                        &target.id,
                        started.elapsed().as_secs_f64(),
                        "config.write_failed",
                    );
                    if is_queued_target {
                        complete_backup_queue_target(&backup_queue, &status_state, &target.id);
                    }
                    lifecycle.finish_task();
                    continue;
                }
                let provider = settings_config::endpoint_provider(&ep.id);

                let storage = tokio::select! {
                    _ = task_cancel.cancelled() => Err(televy_backup_core::Error::Cancelled),
                    result = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
                        provider,
                        api_id: settings.telegram.mtproto.api_id,
                        api_hash: api_hash.clone(),
                        bot_token: bot_token.clone(),
                        chat_id: ep.chat_id.clone(),
                        session,
                        cache_dir,
                        min_delay_ms: Some(ep.rate_limit.min_delay_ms as u64),
                        max_concurrent_uploads: Some(ep.rate_limit.max_concurrent_uploads as usize),
                        helper_path: None,
                    }) => result,
                };

                let storage = match storage {
                    Ok(storage) => storage,
                    Err(error) => {
                        let cancelled = matches!(&error, televy_backup_core::Error::Cancelled);
                        tracing::error!(
                            event = "run.finish",
                            kind = "backup",
                            run_id = %task_id,
                            task_id = %task_id,
                            status = if cancelled { "cancelled" } else { "failed" },
                            error_code = if cancelled { "task.cancelled" } else { error.code() },
                            error_message = %error,
                            target_id = %target.id,
                            endpoint_id = %ep.id,
                            "run.finish"
                        );
                        if cancelled {
                            backup_task_cancelled(
                                &status_state,
                                &target.id,
                                started.elapsed().as_secs_f64(),
                            );
                        } else {
                            backup_task_failed(
                                &status_state,
                                &target.id,
                                started.elapsed().as_secs_f64(),
                                error.code(),
                            );
                            if is_queued_target {
                                complete_backup_queue_target(
                                    &backup_queue,
                                    &status_state,
                                    &target.id,
                                );
                            }
                        }
                        lifecycle.finish_task();
                        continue;
                    }
                };

                storage_by_endpoint.insert(ep.id.clone(), storage);
            }

            let storage = match storage_by_endpoint.get(&ep.id) {
                Some(s) => s,
                None => {
                    tracing::error!(
                        event = "run.finish",
                        kind = "backup",
                        run_id = %task_id,
                        task_id = %task_id,
                        status = "failed",
                        error_code = "telegram.storage_unavailable",
                        target_id = %target.id,
                        endpoint_id = %ep.id,
                        "run.finish"
                    );
                    backup_task_failed(
                        &status_state,
                        &target.id,
                        started.elapsed().as_secs_f64(),
                        "telegram.storage_unavailable",
                    );
                    if is_queued_target {
                        complete_backup_queue_target(&backup_queue, &status_state, &target.id);
                    }
                    lifecycle.finish_task();
                    continue;
                }
            };

            // Only consume the schedule slot once all required config/secrets are available
            // and the endpoint storage is ready.
            match scheduled_slot {
                ScheduleSlot::Hourly(key) => state.last_hourly = Some(key),
                ScheduleSlot::Daily(key) => state.last_daily = Some(key),
                ScheduleSlot::Manual => {
                    // A queued manual batch consumes an overlapping schedule slot too, avoiding an
                    // immediate duplicate scheduled run within the same minute.
                    let eff = settings_config::effective_schedule(
                        &settings.schedule,
                        target.schedule.as_ref(),
                    );
                    if eff.enabled {
                        match eff.kind.as_str() {
                            "hourly" => {
                                if now.minute() == eff.hourly_minute as u32 {
                                    let key = (now.year(), now.month(), now.day(), now.hour());
                                    state.last_hourly = Some(key);
                                }
                            }
                            "daily" => {
                                if let Ok((hh, mm)) = parse_hhmm(&eff.daily_at)
                                    && now.hour() == hh as u32
                                    && now.minute() == mm as u32
                                {
                                    let key = (now.year(), now.month(), now.day());
                                    state.last_daily = Some(key);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            let label = match scheduled_slot {
                ScheduleSlot::Manual => "manual".to_string(),
                _ => {
                    if target.label.trim().is_empty() {
                        "scheduled".to_string()
                    } else {
                        target.label.clone()
                    }
                }
            };

            let db_path = index_dir.join(format!("index.{}.sqlite", ep.id));
            let filemap_dir = index_dir.join("filemaps").join(&ep.id);
            let dedupe_db_path = index_dir
                .join("dedupe")
                .join(format!("dedupe.{}.sqlite", ep.id));
            let dedupe_pending_db_path = index_dir
                .join("dedupe")
                .join(format!("pending.{}.sqlite", ep.id));

            let sink = StatusProgressSink {
                target_id: target.id.clone(),
                state: status_state.clone(),
            };
            let progress_sink = Some(&sink as &dyn ProgressSink);
            let quick_stats_cancel = task_cancel.clone();
            let quick_stats_cancel_for_task = quick_stats_cancel.clone();
            let prepare_res = tokio::try_join!(
                preflight_remote_first_index_sync_daemon(
                    storage,
                    &master_key,
                    &target.id,
                    &target.source_path,
                    &db_path,
                    &filemap_dir,
                    &dedupe_db_path,
                    is_likely_private_chat_id(&ep.chat_id),
                    progress_sink,
                    &task_cancel,
                ),
                async {
                    match preflight_local_quick_stats_daemon(
                        Path::new(&target.source_path),
                        progress_sink,
                        Some(quick_stats_cancel_for_task),
                    )
                    .await
                    {
                        Ok(stats) => Ok(Some(stats)),
                        Err(e) => {
                            tracing::warn!(
                                event = "prepare.local_quick_stats_failed",
                                target_id = %target.id,
                                source_path = %target.source_path,
                                error_code = e.code(),
                                error_message = %e,
                                "prepare.local_quick_stats_failed"
                            );
                            Ok(None)
                        }
                    }
                }
            );

            let result = match prepare_res {
                Ok((remote_dedupe, quick_stats)) => {
                    let cfg = BackupConfig {
                        endpoint_db_path: db_path.clone(),
                        filemap_dir: filemap_dir.clone(),
                        dedupe_db_path: dedupe_db_path.clone(),
                        dedupe_pending_db_path: dedupe_pending_db_path.clone(),
                        source_path: PathBuf::from(&target.source_path),
                        label: label.clone(),
                        chunking: ChunkingConfig {
                            min_bytes: settings.chunking.min_bytes,
                            avg_bytes: settings.chunking.avg_bytes,
                            max_bytes: settings.chunking.max_bytes,
                        },
                        rate_limit: ep.rate_limit.clone(),
                        master_key,
                        snapshot_id: None,
                        keep_last_snapshots: settings.retention.keep_last_snapshots,
                        remote_dedupe,
                    };
                    let opts = BackupOptions {
                        cancel: Some(&task_cancel),
                        progress: progress_sink,
                        source_quick_stats: quick_stats,
                    };
                    televy_backup_core::run_backup_with(storage, cfg, opts).await
                }
                Err(e) => {
                    quick_stats_cancel.cancel();
                    Err(e)
                }
            };
            let duration_seconds = started.elapsed().as_secs_f64();

            match result {
                Ok(res) => {
                    // Strict remote gating: if bootstrap update fails, the overall run is failed.
                    let bootstrap_update = if is_likely_private_chat_id(&ep.chat_id) {
                        tracing::warn!(
                            event = "bootstrap.skipped",
                            reason = "unsupported_private_chat",
                            chat_id = %ep.chat_id,
                            "bootstrap catalog requires pinning; use a group/channel (e.g. -100...) or @username chat id"
                        );
                        Ok(())
                    } else {
                        tokio::select! {
                            _ = task_cancel.cancelled() => Err(televy_backup_core::Error::Cancelled),
                            update = async {
                                let pool = televy_backup_core::index_db::open_index_db(&db_path).await?;

                                let row = sqlx::query(
                                    "SELECT manifest_object_id FROM remote_indexes WHERE snapshot_id = ? AND provider = ? LIMIT 1",
                                )
                                .bind(&res.snapshot_id)
                                .bind(storage.provider())
                                .fetch_one(&pool)
                                .await?;
                                let filemap_manifest_object_id: String = row.get("manifest_object_id");

                                let endpoint_index_id = match sqlx::query(
                                    "SELECT value FROM endpoint_state WHERE key = ? LIMIT 1",
                                )
                                .bind(televy_backup_core::index_sync::ENDPOINT_STATE_ENDPOINT_INDEX_ID_KEY)
                                .fetch_optional(&pool)
                                .await?
                                {
                                    Some(r) => r.get::<String, _>("value"),
                                    None => televy_backup_core::bootstrap::endpoint_index_id_for_storage(
                                        storage,
                                    )?,
                                };

                                let endpoint_manifest_object_id = sqlx::query(
                                    "SELECT value FROM endpoint_state WHERE key = ? LIMIT 1",
                                )
                                .bind(
                                    televy_backup_core::index_sync::ENDPOINT_STATE_ENDPOINT_MANIFEST_OBJECT_ID_KEY,
                                )
                                .fetch_optional(&pool)
                                .await?
                                .map(|r| r.get::<String, _>("value"))
                                .ok_or_else(|| televy_backup_core::Error::Integrity {
                                    message: "missing endpoint_state.endpoint_manifest_object_id after backup".to_string(),
                                })?;

                                let endpoint_dedupe_id =
                                    televy_backup_core::dedupe_catalog::endpoint_dedupe_id_for_storage(
                                        storage,
                                    )?;
                                let dedupe_catalog_object_id =
                                    televy_backup_core::index_sync::endpoint_state_get(
                                        &dedupe_db_path,
                                        televy_backup_core::index_sync::ENDPOINT_STATE_DEDUPE_CATALOG_OBJECT_ID_KEY,
                                    )
                                    .await?
                                    .ok_or_else(|| televy_backup_core::Error::Integrity {
                                        message: "missing endpoint_state.dedupe_catalog_object_id after backup".to_string(),
                                    })?;

                                bootstrap::update_remote_latest(
                                    storage,
                                    &master_key,
                                    Some(bootstrap::BootstrapEndpointLatest {
                                        endpoint_index_id,
                                        manifest_object_id: endpoint_manifest_object_id,
                                    }),
                                    Some(bootstrap::BootstrapEndpointDedupeLatest {
                                        endpoint_dedupe_id,
                                        catalog_object_id: dedupe_catalog_object_id,
                                    }),
                                    &target.id,
                                    &target.source_path,
                                    &label,
                                    &res.snapshot_id,
                                    &filemap_manifest_object_id,
                                )
                                .await
                            } => update,
                        }
                    };

                    match bootstrap_update {
                        Ok(()) => {
                            tracing::warn!(
                                event = "run.finish",
                                kind = "backup",
                                run_id = %task_id,
                                task_id = %task_id,
                                status = "succeeded",
                                duration_seconds,
                                snapshot_id = %res.snapshot_id,
                                files_indexed = res.files_indexed,
                                chunks_uploaded = res.chunks_uploaded,
                                data_objects_uploaded = res.data_objects_uploaded,
                                data_objects_estimated_without_pack = res.data_objects_estimated_without_pack,
                                bytes_uploaded = res.bytes_uploaded,
                                bytes_deduped = res.bytes_deduped,
                                index_parts = res.index_parts,
                                "run.finish"
                            );

                            backup_task_succeeded(
                                &status_state,
                                &target.id,
                                duration_seconds,
                                res.files_indexed,
                                res.bytes_uploaded,
                                res.bytes_deduped,
                            );
                        }
                        Err(e) => {
                            if matches!(&e, televy_backup_core::Error::Cancelled) {
                                tracing::warn!(
                                    event = "run.finish",
                                    kind = "backup",
                                    run_id = %task_id,
                                    task_id = %task_id,
                                    status = "cancelled",
                                    duration_seconds,
                                    target_id = %target.id,
                                    "run.finish"
                                );
                                backup_task_cancelled(&status_state, &target.id, duration_seconds);
                            } else {
                                tracing::error!(
                                    event = "bootstrap.update_failed",
                                    target_id = %target.id,
                                    endpoint_id = %ep.id,
                                    error_code = e.code(),
                                    error_message = %e,
                                    "bootstrap.update_failed"
                                );
                                tracing::error!(
                                    event = "run.finish",
                                    kind = "backup",
                                    run_id = %task_id,
                                    task_id = %task_id,
                                    status = "failed",
                                    duration_seconds,
                                    error_code = e.code(),
                                    error_message = %e,
                                    "run.finish"
                                );
                                backup_task_failed(
                                    &status_state,
                                    &target.id,
                                    duration_seconds,
                                    e.code(),
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    if matches!(&e, televy_backup_core::Error::Cancelled) {
                        tracing::warn!(
                            event = "run.finish",
                            kind = "backup",
                            run_id = %task_id,
                            task_id = %task_id,
                            status = "cancelled",
                            duration_seconds,
                            target_id = %target.id,
                            "run.finish"
                        );
                        backup_task_cancelled(&status_state, &target.id, duration_seconds);
                    } else {
                        tracing::error!(
                            event = "run.finish",
                            kind = "backup",
                            run_id = %task_id,
                            task_id = %task_id,
                            status = "failed",
                            duration_seconds,
                            error_code = e.code(),
                            error_message = %e,
                            "run.finish"
                        );

                        backup_task_failed(&status_state, &target.id, duration_seconds, e.code());
                    }
                }
            }

            if let Some(bytes) = storage.session_bytes() {
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                if let Some(store) = secrets_store.as_mut() {
                    let should_write = store
                        .get(&ep.mtproto.session_key)
                        .is_none_or(|v| v != b64.as_str());
                    if should_write {
                        store.set(&ep.mtproto.session_key, b64);
                        if let Err(e) = televy_backup_core::secrets::save_secrets_store(
                            &secrets_path,
                            &vault_key,
                            store,
                        ) {
                            tracing::warn!(
                                event = "secrets.session_persist_failed",
                                error = %e,
                                "failed to persist mtproto session"
                            );
                        }
                    }
                }
            }
            lifecycle.finish_task();

            if is_queued_target {
                complete_backup_queue_target(&backup_queue, &status_state, &target.id);
            }

            if lifecycle.is_shutdown_requested() {
                break;
            }
        }

        clear_mtproto_storage_cache(&mut storage_by_endpoint, "idle_loop_end").await;
        if backup_queue
            .lock()
            .map(|queue| queue.has_work())
            .unwrap_or(false)
        {
            continue;
        }
        let shutdown_token = lifecycle.shutdown_token();
        tokio::select! {
            _ = sleep(Duration::from_secs(1)) => {}
            _ = backup_queue_notify.notified() => {}
            _ = shutdown_token.cancelled() => break,
        }
    }

    clear_mtproto_storage_cache(&mut storage_by_endpoint, "daemon_shutdown").await;
    tracing::info!(
        event = "daemon.shutdown_complete",
        "daemon.shutdown_complete"
    );
    Ok(())
}

async fn clear_mtproto_storage_cache(
    storage_by_endpoint: &mut HashMap<String, TelegramMtProtoStorage>,
    reason: &str,
) {
    if storage_by_endpoint.is_empty() {
        return;
    }

    tracing::info!(
        event = "mtproto.storage_cache.clear",
        reason,
        endpoint_count = storage_by_endpoint.len(),
        "mtproto.storage_cache.clear"
    );

    let drained = std::mem::take(storage_by_endpoint);
    let _ = tokio::task::spawn_blocking(move || drop(drained)).await;
}

#[allow(clippy::too_many_arguments)]
async fn preflight_remote_first_index_sync_daemon(
    storage: &TelegramMtProtoStorage,
    master_key: &[u8; 32],
    target_id: &str,
    source_path: &str,
    local_endpoint_db: &Path,
    filemap_dir: &Path,
    local_dedupe_db: &Path,
    is_private_chat: bool,
    sink: Option<&dyn ProgressSink>,
    cancel: &CancellationToken,
) -> televy_backup_core::Result<televy_backup_core::RemoteDedupeMode> {
    if cancel.is_cancelled() {
        return Err(televy_backup_core::Error::Cancelled);
    }

    tokio::select! {
        _ = cancel.cancelled() => Err(televy_backup_core::Error::Cancelled),
        result = preflight_remote_first_index_sync_daemon_inner(
            storage,
            master_key,
            target_id,
            source_path,
            local_endpoint_db,
            filemap_dir,
            local_dedupe_db,
            is_private_chat,
            sink,
        ) => result,
    }
}

#[allow(clippy::too_many_arguments)]
async fn preflight_remote_first_index_sync_daemon_inner(
    storage: &TelegramMtProtoStorage,
    master_key: &[u8; 32],
    target_id: &str,
    source_path: &str,
    local_endpoint_db: &Path,
    filemap_dir: &Path,
    local_dedupe_db: &Path,
    is_private_chat: bool,
    sink: Option<&dyn ProgressSink>,
) -> televy_backup_core::Result<televy_backup_core::RemoteDedupeMode> {
    let started = Instant::now();
    tracing::debug!(event = "phase.start", phase = "index_sync", "phase.start");
    if let Some(sink) = sink {
        sink.on_progress(TaskProgress {
            phase: "index_sync".to_string(),
            ..Default::default()
        });
    }

    if is_private_chat {
        tracing::warn!(
            event = "index_sync.skipped",
            reason = "unsupported_private_chat",
            "index_sync requires pinned bootstrap catalog; use a group/channel (e.g. -100...) or @username chat id"
        );
        tracing::debug!(
            event = "phase.finish",
            phase = "index_sync",
            duration_ms = started.elapsed().as_millis() as u64,
            index_source = "skipped",
            reason = "unsupported_chat",
            "phase.finish"
        );
        return Ok(televy_backup_core::RemoteDedupeMode::Disabled);
    }

    std::fs::create_dir_all(filemap_dir)?;

    // Strict remote gating: Telegram errors in bootstrap/index fetch are fatal; only "bootstrap is
    // missing" is allowed (first initialization / user pinned something else).
    let catalog = bootstrap::load_remote_catalog(storage, master_key).await?;

    // 1) Sync the endpoint DB from bootstrap.endpointLatest (if present).
    if let Some(endpoint_latest) = catalog.as_ref().and_then(|c| c.endpoint_latest.clone()) {
        let already_synced =
            televy_backup_core::index_sync::local_endpoint_db_matches_remote_latest(
                local_endpoint_db,
                &endpoint_latest.manifest_object_id,
            )
            .await?;

        if !already_synced {
            let provider = storage.provider();
            let stats = televy_backup_core::remote_index_db::download_and_write_index_db_atomic(
                storage,
                &endpoint_latest.endpoint_index_id,
                &endpoint_latest.manifest_object_id,
                master_key,
                local_endpoint_db,
                None,
                Some(provider),
                sink,
            )
            .await?;

            // Record the pointer locally so future runs can skip redundant downloads.
            televy_backup_core::index_sync::endpoint_state_set(
                local_endpoint_db,
                televy_backup_core::index_sync::ENDPOINT_STATE_ENDPOINT_INDEX_ID_KEY,
                &endpoint_latest.endpoint_index_id,
            )
            .await?;
            televy_backup_core::index_sync::endpoint_state_set(
                local_endpoint_db,
                televy_backup_core::index_sync::ENDPOINT_STATE_ENDPOINT_MANIFEST_OBJECT_ID_KEY,
                &endpoint_latest.manifest_object_id,
            )
            .await?;

            tracing::debug!(
                event = "index_sync.endpoint.downloaded",
                target_id,
                bytes_downloaded = stats.bytes_downloaded,
                bytes_written = stats.bytes_written,
                "index_sync.endpoint.downloaded"
            );
        }
    } else if catalog.is_none() {
        tracing::debug!(
            event = "index_sync.skipped",
            reason = "bootstrap_missing",
            target_id,
            "no pinned bootstrap catalog (first init); continue without remote endpoint sync"
        );
    }

    // 2) Sync the remote dedupe catalog/base/deltas (if present).
    let remote_dedupe = if let Some(dedupe_latest) = catalog
        .as_ref()
        .and_then(|c| c.endpoint_dedupe_latest.clone())
    {
        let already_synced =
            televy_backup_core::dedupe_sync::local_dedupe_db_matches_remote_latest(
                local_dedupe_db,
                &dedupe_latest.catalog_object_id,
            )
            .await?;
        if !already_synced {
            let provider = storage.provider();
            let stats = televy_backup_core::dedupe_sync::materialize_remote_dedupe_db(
                storage,
                master_key,
                &dedupe_latest.endpoint_dedupe_id,
                &dedupe_latest.catalog_object_id,
                local_dedupe_db,
                Some(provider),
                sink,
            )
            .await?;
            tracing::debug!(
                event = "index_sync.dedupe.downloaded",
                target_id,
                base_bytes_downloaded = stats.base_bytes_downloaded,
                delta_bytes_downloaded = stats.delta_bytes_downloaded,
                "index_sync.dedupe.downloaded"
            );
        }

        televy_backup_core::RemoteDedupeMode::Incremental {
            endpoint_dedupe_id: dedupe_latest.endpoint_dedupe_id,
            catalog_object_id: dedupe_latest.catalog_object_id,
        }
    } else {
        let endpoint_dedupe_id =
            televy_backup_core::dedupe_catalog::endpoint_dedupe_id_for_storage(storage)?;
        televy_backup_core::RemoteDedupeMode::Enable { endpoint_dedupe_id }
    };

    // 3) Fail fast: if a base snapshot exists and its filemap DB is not cached locally, download it
    // now (so the scan phase won't hit a remote error hours later).
    if local_endpoint_db.exists() {
        let pool = televy_backup_core::index_db::open_index_db(local_endpoint_db).await?;

        let provider = storage.provider();
        let kind = provider.split(['/', ':']).next().unwrap_or(provider).trim();
        let like = format!("{kind}%");
        let base_row = sqlx::query(
            r#"
            SELECT s.snapshot_id as snapshot_id
            FROM snapshots s
            JOIN remote_indexes ri ON ri.snapshot_id = s.snapshot_id
            WHERE s.source_path = ?
              AND (ri.provider = ? OR ri.provider LIKE ?)
            ORDER BY s.created_at DESC
            LIMIT 1
            "#,
        )
        .bind(source_path)
        .bind(provider)
        .bind(&like)
        .fetch_optional(&pool)
        .await?;

        if let Some(base_row) = base_row {
            let base_snapshot_id: String = base_row.get("snapshot_id");
            let cached_path = filemap_dir.join(format!("{base_snapshot_id}.sqlite"));
            if !cached_path.exists() {
                let row = sqlx::query(
                    "SELECT provider, manifest_object_id FROM remote_indexes WHERE snapshot_id = ? LIMIT 1",
                )
                .bind(&base_snapshot_id)
                .fetch_optional(&pool)
                .await?;

                let row = row.ok_or_else(|| televy_backup_core::Error::Integrity {
                    message: format!(
                        "base snapshot missing remote index pointer: base_snapshot_id={base_snapshot_id}"
                    ),
                })?;

                let row_provider: String = row.get("provider");
                let manifest_object_id: String = row.get("manifest_object_id");
                let row_kind = row_provider
                    .split(['/', ':'])
                    .next()
                    .unwrap_or(&row_provider)
                    .trim();
                if row_provider == provider || row_kind == kind {
                    televy_backup_core::remote_index_db::download_and_write_index_db_atomic(
                        storage,
                        &base_snapshot_id,
                        &manifest_object_id,
                        master_key,
                        &cached_path,
                        None,
                        Some(provider),
                        None,
                    )
                    .await?;
                }
            }
        }
    }

    tracing::debug!(
        event = "phase.finish",
        phase = "index_sync",
        duration_ms = started.elapsed().as_millis() as u64,
        target_id,
        "phase.finish"
    );

    Ok(remote_dedupe)
}

async fn preflight_local_quick_stats_daemon(
    source_path: &Path,
    sink: Option<&dyn ProgressSink>,
    cancel: Option<CancellationToken>,
) -> televy_backup_core::Result<SourceQuickStats> {
    if let Some(sink) = sink {
        sink.on_progress(TaskProgress {
            phase: "prepare".to_string(),
            ..Default::default()
        });
    }

    let source_path = source_path.to_path_buf();
    let cancel_for_task = cancel;
    let stats = tokio::task::spawn_blocking(move || {
        televy_backup_core::compute_source_quick_stats(&source_path, cancel_for_task.as_ref())
    })
    .await
    .map_err(|e| televy_backup_core::Error::InvalidConfig {
        message: format!("prepare local stats aborted: {e}"),
    })??;

    if let Some(sink) = sink {
        sink.on_progress(TaskProgress {
            phase: "prepare".to_string(),
            source_files_total: Some(stats.files_total),
            source_bytes_total: Some(stats.bytes_total),
            ..Default::default()
        });
    }

    Ok(stats)
}

fn is_likely_private_chat_id(chat_id: &str) -> bool {
    let s = chat_id.trim();
    let Ok(id) = s.parse::<i64>() else {
        return false;
    };
    !s.starts_with("-100") && id > 0
}

fn parse_hhmm(s: &str) -> Result<(u8, u8), Box<dyn std::error::Error>> {
    let (hh, mm) = s.split_once(':').ok_or("daily_at must be HH:MM")?;
    let hh: u8 = hh.parse()?;
    let mm: u8 = mm.parse()?;
    Ok((hh, mm))
}

fn default_config_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home)
        .join("Library")
        .join("Application Support")
        .join("TelevyBackup")
}

fn default_data_dir() -> PathBuf {
    default_config_dir()
}

const MASTER_KEY_KEY: &str = "televybackup.master_key";
static CONFIG_ROOT_CACHE: OnceLock<PathBuf> = OnceLock::new();
static VAULT_KEY_CACHE: OnceLock<Mutex<Option<[u8; 32]>>> = OnceLock::new();
static VAULT_KEY_LOAD_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

pub(crate) fn get_cached_vault_key() -> Option<[u8; 32]> {
    let lock = VAULT_KEY_CACHE.get_or_init(|| Mutex::new(None));
    lock.lock().ok().and_then(|g| *g)
}

pub(crate) fn set_cached_vault_key(key: [u8; 32]) {
    let lock = VAULT_KEY_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = lock.lock() {
        *g = Some(key);
    }
}

pub(crate) fn clear_cached_vault_key() {
    let lock = VAULT_KEY_CACHE.get_or_init(|| Mutex::new(None));
    if let Ok(mut g) = lock.lock() {
        *g = None;
    }
}

fn get_secret_from_store(
    store: &televy_backup_core::secrets::SecretsStore,
    key: &str,
) -> Option<String> {
    store.get(key).map(|s| s.to_string())
}

fn decode_base64_32(b64: &str) -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let bytes = base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?;
    let arr: [u8; 32] = bytes.try_into().map_err(|_| "invalid key length")?;
    Ok(arr)
}

fn initialize_dev_master_key_if_missing(
    config_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    if !keychain_disabled() {
        return Ok(());
    }

    let secrets_path = televy_backup_core::secrets::secrets_path(config_root);
    if secrets_path.exists() {
        return Ok(());
    }

    let vault_key = load_or_create_vault_key_uncached()?;
    let mut store = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)?;
    if store.contains_key(MASTER_KEY_KEY) {
        return Ok(());
    }

    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| std::io::Error::other(format!("getrandom failed: {e}")))?;
    store.set(
        MASTER_KEY_KEY,
        televy_backup_core::secrets::vault_key_to_base64(&bytes),
    );
    televy_backup_core::secrets::save_secrets_store(&secrets_path, &vault_key, &store)?;
    set_cached_vault_key(vault_key);
    Ok(())
}

fn keychain_disabled() -> bool {
    matches!(
        std::env::var("TELEVYBACKUP_DISABLE_KEYCHAIN").as_deref(),
        Ok("1")
    )
}

#[cfg(target_os = "macos")]
pub(crate) fn keychain_get_secret(key: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    use security_framework::passwords::{PasswordOptions, generic_password};

    if keychain_disabled() {
        return Err("keychain disabled".into());
    }

    let opts = PasswordOptions::new_generic_password(televy_backup_core::APP_NAME, key);
    match generic_password(opts) {
        Ok(bytes) => Ok(Some(String::from_utf8(bytes)?)),
        Err(e) => {
            if is_keychain_not_found(&e) {
                Ok(None)
            } else {
                Err(Box::new(e))
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn keychain_get_secret(
    _key: &str,
) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Err("keychain only supported on macOS".into())
}

#[cfg(target_os = "macos")]
fn keychain_set_secret(key: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    use security_framework::passwords::set_generic_password;
    if keychain_disabled() {
        return Err("keychain disabled".into());
    }
    set_generic_password(televy_backup_core::APP_NAME, key, value.as_bytes())?;
    Ok(())
}

#[cfg(target_os = "macos")]
pub(crate) fn keychain_delete_secret(key: &str) -> Result<bool, Box<dyn std::error::Error>> {
    use security_framework::passwords::delete_generic_password;

    if keychain_disabled() {
        return Err("keychain disabled".into());
    }

    match delete_generic_password(televy_backup_core::APP_NAME, key) {
        Ok(()) => Ok(true),
        Err(e) => {
            if is_keychain_not_found(&e) {
                return Ok(false);
            }
            Err(Box::new(e))
        }
    }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn keychain_delete_secret(_key: &str) -> Result<bool, Box<dyn std::error::Error>> {
    Err("keychain only supported on macOS".into())
}

#[cfg(target_os = "macos")]
fn is_keychain_not_found(e: &security_framework::base::Error) -> bool {
    // errSecItemNotFound
    e.code() == -25300
}

pub(crate) fn load_or_create_vault_key() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    // IMPORTANT: Keychain access can block (e.g. waiting for user authentication / permission).
    // Never do that on the main async flow, otherwise the daemon can't serve status and control IPC.
    //
    // We only return a cached key here. A background loader (spawned from `main`) is responsible
    // for eventually populating `VAULT_KEY_CACHE` when Keychain is available.
    if let Some(key) = get_cached_vault_key() {
        return Ok(key);
    }

    // File-based backend (dev / keychain-disabled) is safe to load synchronously.
    if keychain_disabled() {
        let key = load_or_create_vault_key_uncached()?;
        set_cached_vault_key(key);
        return Ok(key);
    }

    Err("vault key unavailable (waiting for Keychain)".into())
}

pub(crate) fn load_or_create_vault_key_uncached() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    // Serialize Keychain (and vault key file) access to avoid spawning multiple concurrent
    // authorization prompts when multiple clients call into the vault IPC at the same time.
    let lock = VAULT_KEY_LOAD_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock
        .lock()
        .map_err(|_| std::io::Error::other("vault key load lock poisoned"))?;

    let config_root = CONFIG_ROOT_CACHE
        .get()
        .cloned()
        .unwrap_or_else(default_config_dir);

    let key_file_path = std::env::var("TELEVYBACKUP_VAULT_KEY_FILE")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| televy_backup_core::secrets::vault_key_file_path(&config_root));

    if let Ok(b64) = std::env::var("TELEVYBACKUP_VAULT_KEY_B64") {
        let key = televy_backup_core::secrets::vault_key_from_base64(b64.trim())?;
        televy_backup_core::secrets::write_vault_key_file_private(&key_file_path, &key)?;
        return Ok(key);
    }

    if keychain_disabled() {
        match televy_backup_core::secrets::read_vault_key_file(&key_file_path) {
            Ok(key) => return Ok(key),
            Err(televy_backup_core::secrets::SecretsStoreError::Io(e))
                if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(Box::new(e)),
        }

        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)
            .map_err(|e| std::io::Error::other(format!("getrandom failed: {e}")))?;
        televy_backup_core::secrets::write_vault_key_file_private(&key_file_path, &bytes)?;
        return Ok(bytes);
    }

    #[cfg(target_os = "macos")]
    {
        let secrets_path = televy_backup_core::secrets::secrets_path(&config_root);

        // Migration/fallback: if a secrets store already exists and a legacy vault.key file is
        // present, prefer using it to unlock secrets. This avoids getting "stuck" when the user
        // previously ran with file backend (`TELEVYBACKUP_DISABLE_KEYCHAIN=1`) and later enables
        // Keychain with an empty vault key item.
        if secrets_path.exists()
            && key_file_path.exists()
            && let Ok(key) = televy_backup_core::secrets::read_vault_key_file(&key_file_path)
            // Validate that this key can decrypt the existing secrets store.
            && televy_backup_core::secrets::load_secrets_store(&secrets_path, &key).is_ok()
        {
            return Ok(key);
        }

        let existing = keychain_get_secret(televy_backup_core::secrets::VAULT_KEY_KEY)?
            .map(|s| s.trim().to_string());
        if let Some(b64) = existing {
            let key = televy_backup_core::secrets::vault_key_from_base64(&b64)?;
            if secrets_path.exists()
                && televy_backup_core::secrets::load_secrets_store(&secrets_path, &key).is_err()
            {
                return Err(Box::new(std::io::Error::other(
                    "vault key mismatch for existing secrets store (Keychain item may be wrong); restore/migrate the correct vault key",
                )));
            }
            return Ok(key);
        }

        // If a secrets store already exists, creating a brand-new vault key would only make it
        // unreadable. Require the user to restore/migrate the vault key first.
        if secrets_path.exists() {
            return Err("vault key missing for existing secrets store (Keychain empty)".into());
        }

        let mut bytes = [0u8; 32];
        getrandom::getrandom(&mut bytes)
            .map_err(|e| std::io::Error::other(format!("getrandom failed: {e}")))?;
        let b64 = televy_backup_core::secrets::vault_key_to_base64(&bytes);
        keychain_set_secret(televy_backup_core::secrets::VAULT_KEY_KEY, &b64)?;
        Ok(bytes)
    }

    #[cfg(not(target_os = "macos"))]
    {
        Err("vault key unavailable (keychain unsupported)".into())
    }
}
