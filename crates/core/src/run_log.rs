use std::fs::OpenOptions;
use std::io::{LineWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::Utc;
use tracing::Dispatch;
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
}
