use std::fs::{File, OpenOptions};
use std::io::{LineWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use tracing::{Dispatch, info, warn};
use tracing_subscriber::{EnvFilter, Registry, layer::SubscriberExt, reload};

static RUN_LOGGER: OnceLock<RunLogger> = OnceLock::new();
static RUN_LOG_DISPATCH: OnceLock<Dispatch> = OnceLock::new();
static RUN_LOG_FILTER: OnceLock<reload::Handle<EnvFilter, Registry>> = OnceLock::new();
static TRACING_INIT: OnceLock<()> = OnceLock::new();

#[derive(Debug)]
struct RunState {
    // Use line-buffering so the UI can observe run logs immediately after a run finishes,
    // even if the daemon keeps the file open a little longer for follow-up steps (bootstrap update, etc).
    writer: Option<LineWriter<std::fs::File>>,
}

#[derive(Debug)]
struct RunLogger {
    state: Mutex<RunState>,
}

impl RunLogger {
    fn new() -> Self {
        Self {
            state: Mutex::new(RunState { writer: None }),
        }
    }

    fn start(&self, path: &Path) -> std::io::Result<()> {
        let mut guard = self.state.lock().expect("run log mutex poisoned");
        if guard.writer.is_some() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                "run log already active",
            ));
        }

        let file = OpenOptions::new().create_new(true).write(true).open(path)?;
        lock_shared(&file)?;
        guard.writer = Some(LineWriter::new(file));
        Ok(())
    }

    fn finish(&self) -> std::io::Result<()> {
        let mut guard = self.state.lock().expect("run log mutex poisoned");
        let mut writer = match guard.writer.take() {
            Some(writer) => writer,
            None => return Ok(()),
        };
        writer.flush()?;
        writer.get_ref().sync_all()?;
        Ok(())
    }

    fn flush_active(&self) -> std::io::Result<()> {
        let mut guard = self.state.lock().expect("run log mutex poisoned");
        let Some(writer) = guard.writer.as_mut() else {
            return Ok(());
        };
        writer.flush()?;
        writer.get_ref().sync_all()
    }
}

enum RunLogWriter<'a> {
    Sink(std::io::Sink),
    Guard(MutexGuard<'a, RunState>),
}

impl Write for RunLogWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Self::Sink(sink) => sink.write(buf),
            Self::Guard(guard) => guard
                .writer
                .as_mut()
                .expect("writer missing while run log active")
                .write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Self::Sink(sink) => sink.flush(),
            Self::Guard(guard) => guard
                .writer
                .as_mut()
                .expect("writer missing while run log active")
                .flush(),
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for RunLogger {
    type Writer = RunLogWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        let guard = self.state.lock().expect("run log mutex poisoned");
        if guard.writer.is_some() {
            RunLogWriter::Guard(guard)
        } else {
            RunLogWriter::Sink(std::io::sink())
        }
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for &RunLogger {
    type Writer = RunLogWriter<'a>;

    fn make_writer(&'a self) -> Self::Writer {
        (**self).make_writer()
    }
}

pub fn init_run_logging(filter: &str) -> std::io::Result<()> {
    let env_filter = EnvFilter::try_new(filter)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
    TRACING_INIT.get_or_init(|| {
        let logger = RUN_LOGGER.get_or_init(RunLogger::new);
        let (filter_layer, filter_handle) = reload::Layer::new(env_filter.clone());
        let _ = RUN_LOG_FILTER.set(filter_handle);

        let layer = tracing_subscriber::fmt::layer()
            .json()
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
            .with_writer(logger);

        let subscriber = tracing_subscriber::registry()
            .with(filter_layer)
            .with(layer);
        let dispatch = Dispatch::new(subscriber);

        // Keep a handle so `start_run_log` can always install a thread-local dispatcher,
        // even if another test/app already initialized a different global subscriber.
        let _ = RUN_LOG_DISPATCH.set(dispatch.clone());

        // Best-effort global init. If something already set a global dispatcher, we'll fall back
        // to thread-local dispatchers in `start_run_log`.
        let _ = tracing::dispatcher::set_global_default(dispatch);
    });

    RUN_LOG_FILTER
        .get()
        .expect("run log filter missing")
        .reload(env_filter)
        .map_err(std::io::Error::other)
}

pub struct RunLogGuard {
    path: PathBuf,
    log_dir: PathBuf,
    retention: Option<crate::local_settings::LogRetentionSettings>,
    _dispatch: Dispatch,
    _dispatch_guard: tracing::dispatcher::DefaultGuard,
}

impl RunLogGuard {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RunLogGuard {
    fn drop(&mut self) {
        if let Some(logger) = RUN_LOGGER.get()
            && let Err(error) = logger.flush_active()
        {
            warn!(
                event = "log.retention.current_log_flush_failed",
                error = %error,
                "log.retention.current_log_flush_failed"
            );
        }
        if let Some(retention) = self.retention {
            match prune_completed_logs(&self.log_dir, &self.path, retention) {
                Ok(summary) => info!(
                    event = "log.retention.prune",
                    managed_bytes_before = summary.managed_bytes_before,
                    managed_bytes_after = summary.managed_bytes_after,
                    managed_files_before = summary.managed_files_before,
                    managed_files_after = summary.managed_files_after,
                    deleted_files = summary.deleted_files,
                    deleted_bytes = summary.deleted_bytes,
                    deleted_by_age = summary.deleted_by_age,
                    skipped_active_files = summary.skipped_active_files,
                    over_limit = summary.over_limit,
                    "log.retention.prune"
                ),
                Err(error) => warn!(
                    event = "log.retention.prune_failed",
                    error = %error,
                    "log.retention.prune_failed"
                ),
            }
        } else {
            info!(
                event = "log.retention.prune_skipped",
                reason = "invalid_local_settings",
                "log.retention.prune_skipped"
            );
        }
        if let Some(logger) = RUN_LOGGER.get() {
            let _ = logger.finish();
        }
    }
}

pub fn start_run_log(
    kind: &str,
    run_id: &str,
    data_dir: &Path,
    effective_filter: &str,
) -> std::io::Result<RunLogGuard> {
    start_run_log_with_retention(kind, run_id, data_dir, effective_filter, None)
}

pub fn start_run_log_with_retention(
    kind: &str,
    run_id: &str,
    data_dir: &Path,
    effective_filter: &str,
    retention: Option<crate::local_settings::LogRetentionSettings>,
) -> std::io::Result<RunLogGuard> {
    init_run_logging(effective_filter)?;

    match kind {
        "backup" | "restore" | "verify" => {}
        other => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unsupported kind: {other}"),
            ));
        }
    }

    let log_dir = resolve_log_dir(data_dir);
    std::fs::create_dir_all(&log_dir)?;

    let started_at_utc = Utc::now();
    let file_name = format!(
        "sync-{}-{}-{}.ndjson",
        sanitize_filename_component(kind),
        started_at_utc.format("%Y%m%dT%H%M%SZ"),
        sanitize_filename_component(run_id)
    );
    let path = log_dir.join(file_name);

    let logger = RUN_LOGGER.get_or_init(RunLogger::new);
    logger.start(&path)?;

    let dispatch = RUN_LOG_DISPATCH
        .get()
        .expect("run log dispatch missing")
        .clone();
    let dispatch_guard = tracing::dispatcher::set_default(&dispatch);

    Ok(RunLogGuard {
        path,
        log_dir,
        retention,
        _dispatch: dispatch,
        _dispatch_guard: dispatch_guard,
    })
}

pub fn resolve_log_dir(data_dir: &Path) -> PathBuf {
    if let Ok(v) = std::env::var("TELEVYBACKUP_LOG_DIR") {
        return PathBuf::from(v);
    }
    data_dir.join("logs")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ManagedLogUsage {
    pub bytes: u64,
    pub file_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RunLogPruneSummary {
    pub managed_bytes_before: u64,
    pub managed_bytes_after: u64,
    pub managed_files_before: u64,
    pub managed_files_after: u64,
    pub deleted_files: u64,
    pub deleted_bytes: u64,
    pub deleted_by_age: u64,
    pub skipped_active_files: u64,
    pub over_limit: bool,
}

#[derive(Debug)]
struct ManagedLogEntry {
    path: PathBuf,
    started_at: DateTime<Utc>,
    bytes: u64,
}

pub fn managed_log_usage(log_dir: &Path) -> std::io::Result<ManagedLogUsage> {
    let entries = managed_log_entries(log_dir)?;
    Ok(ManagedLogUsage {
        bytes: entries.iter().map(|entry| entry.bytes).sum(),
        file_count: entries.len() as u64,
    })
}

pub fn prune_completed_logs(
    log_dir: &Path,
    current_path: &Path,
    retention: crate::local_settings::LogRetentionSettings,
) -> std::io::Result<RunLogPruneSummary> {
    prune_completed_logs_at(log_dir, current_path, retention, Utc::now())
}

fn prune_completed_logs_at(
    log_dir: &Path,
    current_path: &Path,
    retention: crate::local_settings::LogRetentionSettings,
    now: DateTime<Utc>,
) -> std::io::Result<RunLogPruneSummary> {
    let mut entries = managed_log_entries(log_dir)?;
    entries.sort_by_key(|entry| entry.started_at);

    let managed_bytes_before: u64 = entries.iter().map(|entry| entry.bytes).sum();
    let managed_files_before = entries.len() as u64;
    let mut managed_bytes_after = managed_bytes_before;
    let mut managed_files_after = managed_files_before;
    let mut deleted_files: u64 = 0;
    let mut deleted_bytes: u64 = 0;
    let mut deleted_by_age: u64 = 0;
    let mut skipped_active_files: u64 = 0;
    let max_age = Duration::from_secs(u64::from(retention.max_age_days) * 24 * 60 * 60);

    for entry in &entries {
        let age_expired = now
            .signed_duration_since(entry.started_at)
            .to_std()
            .is_ok_and(|age| age > max_age);
        if age_expired {
            match remove_if_unlocked(entry, current_path)? {
                RemoveResult::Deleted => {
                    managed_bytes_after = managed_bytes_after.saturating_sub(entry.bytes);
                    managed_files_after = managed_files_after.saturating_sub(1);
                    deleted_files += 1;
                    deleted_bytes = deleted_bytes.saturating_add(entry.bytes);
                    deleted_by_age += 1;
                }
                RemoveResult::SkippedActive => skipped_active_files += 1,
                RemoveResult::Current => {}
            }
        }
    }

    if managed_bytes_after > retention.max_total_bytes() {
        for entry in &entries {
            if managed_bytes_after <= retention.max_total_bytes() || !entry.path.exists() {
                continue;
            }
            match remove_if_unlocked(entry, current_path)? {
                RemoveResult::Deleted => {
                    managed_bytes_after = managed_bytes_after.saturating_sub(entry.bytes);
                    managed_files_after = managed_files_after.saturating_sub(1);
                    deleted_files += 1;
                    deleted_bytes = deleted_bytes.saturating_add(entry.bytes);
                }
                RemoveResult::SkippedActive => skipped_active_files += 1,
                RemoveResult::Current => {}
            }
        }
    }

    Ok(RunLogPruneSummary {
        managed_bytes_before,
        managed_bytes_after,
        managed_files_before,
        managed_files_after,
        deleted_files,
        deleted_bytes,
        deleted_by_age,
        skipped_active_files,
        over_limit: managed_bytes_after > retention.max_total_bytes(),
    })
}

fn managed_log_entries(log_dir: &Path) -> std::io::Result<Vec<ManagedLogEntry>> {
    let mut entries = Vec::new();
    let directory = match std::fs::read_dir(log_dir) {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
        Err(error) => return Err(error),
    };
    for entry in directory {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let path = entry.path();
        let Some(started_at) = parse_run_log_timestamp(&path) else {
            continue;
        };
        if !is_completed_run_log(&path)? {
            continue;
        }
        entries.push(ManagedLogEntry {
            bytes: entry.metadata()?.len(),
            path,
            started_at,
        });
    }
    Ok(entries)
}

fn is_completed_run_log(path: &Path) -> std::io::Result<bool> {
    const TAIL_BYTES: u64 = 64 * 1024;

    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let offset = length.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(offset))?;
    let mut tail = String::new();
    file.read_to_string(&mut tail)?;
    Ok(tail.lines().rev().any(|line| {
        serde_json::from_str::<serde_json::Value>(line)
            .ok()
            .and_then(|value| {
                value
                    .get("fields")?
                    .get("event")?
                    .as_str()
                    .map(str::to_owned)
            })
            .is_some_and(|event| event == "run.finish")
    }))
}

fn parse_run_log_timestamp(path: &Path) -> Option<DateTime<Utc>> {
    let file_name = path.file_name()?.to_str()?;
    let remainder = ["backup", "restore", "verify"]
        .iter()
        .find_map(|kind| {
            file_name
                .strip_prefix(&format!("sync-{kind}-"))
                .map(|remainder| (kind, remainder))
        })?
        .1;
    let timestamp = remainder.get(..16)?;
    let suffix = remainder.get(16..)?;
    if !suffix.starts_with('-') || !remainder.ends_with(".ndjson") {
        return None;
    }
    NaiveDateTime::parse_from_str(timestamp, "%Y%m%dT%H%M%SZ")
        .ok()
        .map(|timestamp| timestamp.and_utc())
}

enum RemoveResult {
    Deleted,
    SkippedActive,
    Current,
}

fn remove_if_unlocked(
    entry: &ManagedLogEntry,
    current_path: &Path,
) -> std::io::Result<RemoveResult> {
    if entry.path == current_path {
        return Ok(RemoveResult::Current);
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&entry.path)?;
    if !try_lock_exclusive(&file)? {
        return Ok(RemoveResult::SkippedActive);
    }
    std::fs::remove_file(&entry.path)?;
    Ok(RemoveResult::Deleted)
}

#[cfg(unix)]
fn lock_shared(file: &File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn lock_shared(_file: &File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn try_lock_exclusive(file: &File) -> std::io::Result<bool> {
    use std::os::fd::AsRawFd;

    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    let raw = error.raw_os_error().unwrap_or_default();
    if raw == libc::EWOULDBLOCK || raw == libc::EAGAIN {
        Ok(false)
    } else {
        Err(error)
    }
}

#[cfg(not(unix))]
fn try_lock_exclusive(_file: &File) -> std::io::Result<bool> {
    Ok(true)
}

fn sanitize_filename_component(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '.' | '_' | '-' => c,
            _ => '_',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_log_reloads_filter_between_runs_and_is_ndjson() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let guard = start_run_log(
            "backup",
            "tsk_normal",
            temp.path(),
            crate::local_settings::NORMAL_FILTER,
        )
        .expect("start_run_log");

        let expected_dir = temp.path().join("logs");
        assert_eq!(guard.path().parent(), Some(expected_dir.as_path()));

        // Use WARN so the test remains stable even when the test runner sets RUST_LOG to warn/error.
        tracing::warn!(
            event = "run.start",
            kind = "backup",
            run_id = "tsk_test",
            task_id = "tsk_test",
            "run.start"
        );
        tracing::debug!(target: "sqlx::query", event = "query", "query details");
        tracing::warn!(event = "phase.start", phase = "scan", "phase.start");
        tracing::warn!(event = "phase.finish", phase = "scan", "phase.finish");
        tracing::warn!(
            event = "run.finish",
            kind = "backup",
            run_id = "tsk_test",
            status = "succeeded",
            "run.finish"
        );

        let path = guard.path().to_path_buf();
        drop(guard);

        let text = std::fs::read_to_string(&path).expect("read run log");
        assert!(!text.trim().is_empty(), "run log is empty");

        for line in text.lines() {
            let v: serde_json::Value = serde_json::from_str(line).expect("valid json line");
            let obj = v.as_object().expect("json object");
            assert!(obj.contains_key("timestamp"));
            assert!(obj.contains_key("level"));
            assert!(obj.contains_key("target"));
            assert!(obj.contains_key("fields"));
            let fields = obj
                .get("fields")
                .expect("fields")
                .as_object()
                .expect("fields object");
            assert!(
                fields.contains_key("message")
                    || fields.contains_key("event")
                    || fields.contains_key("summary"),
                "fields missing expected keys (message/event/summary)"
            );
        }
        assert!(!text.contains("query details"));

        let debug_guard = start_run_log("backup", "tsk_debug", temp.path(), "debug")
            .expect("start debug run log");
        tracing::debug!(target: "sqlx::query", event = "query", "query details");
        let debug_path = debug_guard.path().to_path_buf();
        drop(debug_guard);
        assert!(
            std::fs::read_to_string(debug_path)
                .unwrap()
                .contains("query details")
        );
    }

    fn test_retention(
        max_total_gib: u16,
        max_age_days: u16,
    ) -> crate::local_settings::LogRetentionSettings {
        crate::local_settings::LogRetentionSettings {
            max_total_gib,
            max_age_days,
        }
    }

    fn test_now(value: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(value)
            .expect("valid timestamp")
            .with_timezone(&Utc)
    }

    fn create_run_log(dir: &Path, name: &str, bytes: u64) -> PathBuf {
        let path = dir.join(name);
        let mut file = File::create(&path).expect("create run log");
        file.set_len(bytes).expect("size run log");
        file.seek(SeekFrom::End(0)).expect("seek run log end");
        file.write_all(b"\n{\"fields\":{\"event\":\"run.finish\"}}\n")
            .expect("write run finish");
        path
    }

    #[test]
    fn managed_usage_ignores_ui_and_unknown_files() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let logs = temp.path().join("logs");
        std::fs::create_dir(&logs).expect("create logs");
        create_run_log(&logs, "sync-backup-20240102T000000Z-a.ndjson", 17);
        std::fs::write(
            logs.join("sync-restore-20240102T000000Z-incomplete.ndjson"),
            "{\"fields\":{\"event\":\"run.start\"}}\n",
        )
        .expect("write incomplete run log");
        create_run_log(&logs, "ui.log", 100);
        create_run_log(&logs, "unrelated.ndjson", 200);

        assert_eq!(
            managed_log_usage(&logs).expect("managed usage"),
            ManagedLogUsage {
                bytes: 52,
                file_count: 1,
            }
        );
    }

    #[test]
    fn prune_removes_oldest_completed_log_when_capacity_is_exceeded() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let logs = temp.path().join("logs");
        std::fs::create_dir(&logs).expect("create logs");
        let oldest = create_run_log(
            &logs,
            "sync-backup-20240102T000000Z-old.ndjson",
            600 * 1024 * 1024,
        );
        let newest = create_run_log(
            &logs,
            "sync-verify-20240103T000000Z-new.ndjson",
            600 * 1024 * 1024,
        );

        let summary = prune_completed_logs_at(
            &logs,
            &logs.join("sync-backup-20240104T000000Z-current.ndjson"),
            test_retention(1, 365),
            test_now("2024-01-04T00:00:00Z"),
        )
        .expect("prune logs");

        assert!(!oldest.exists());
        assert!(newest.exists());
        assert_eq!(summary.deleted_files, 1);
        assert!(summary.managed_bytes_after <= 1024 * 1024 * 1024);
    }

    #[test]
    fn prune_applies_age_before_capacity_and_skips_current_file() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let logs = temp.path().join("logs");
        std::fs::create_dir(&logs).expect("create logs");
        let expired = create_run_log(&logs, "sync-restore-20200102T000000Z-old.ndjson", 10);
        let current = create_run_log(&logs, "sync-backup-20200103T000000Z-current.ndjson", 10);

        let summary = prune_completed_logs_at(
            &logs,
            &current,
            test_retention(5, 7),
            test_now("2025-01-01T00:00:00Z"),
        )
        .expect("prune logs");

        assert!(!expired.exists());
        assert!(current.exists());
        assert_eq!(summary.deleted_by_age, 1);
    }

    #[cfg(unix)]
    #[test]
    fn prune_skips_a_log_held_by_another_process_lock() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let logs = temp.path().join("logs");
        std::fs::create_dir(&logs).expect("create logs");
        let active = create_run_log(&logs, "sync-verify-20200102T000000Z-active.ndjson", 10);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&active)
            .expect("open active log");
        lock_shared(&lock).expect("take shared lock");

        let summary = prune_completed_logs_at(
            &logs,
            &logs.join("sync-backup-20250101T000000Z-current.ndjson"),
            test_retention(5, 7),
            test_now("2025-01-01T00:00:00Z"),
        )
        .expect("prune logs");

        assert!(active.exists());
        assert_eq!(summary.skipped_active_files, 1);
        assert!(summary.over_limit == false);
    }
}
