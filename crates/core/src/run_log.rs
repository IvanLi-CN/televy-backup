use std::fs::OpenOptions;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use chrono::Utc;
use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

static RUN_LOGGER: OnceLock<RunLogger> = OnceLock::new();
static TRACING_INIT: OnceLock<()> = OnceLock::new();

#[derive(Debug)]
struct RunState {
    writer: Option<BufWriter<std::fs::File>>,
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
        guard.writer = Some(BufWriter::new(file));
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

fn build_env_filter_from(televybackup_log: Option<&str>, rust_log: Option<&str>) -> EnvFilter {
    let default = || EnvFilter::new("debug");

    if let Some(v) = televybackup_log {
        return EnvFilter::try_new(v).unwrap_or_else(|_| default());
    }
    if let Some(v) = rust_log {
        return EnvFilter::try_new(v).unwrap_or_else(|_| default());
    }
    default()
}

fn build_env_filter() -> EnvFilter {
    build_env_filter_from(
        std::env::var("TELEVYBACKUP_LOG").ok().as_deref(),
        std::env::var("RUST_LOG").ok().as_deref(),
    )
}

pub fn init_run_logging() {
    TRACING_INIT.get_or_init(|| {
        let logger = RUN_LOGGER.get_or_init(RunLogger::new);
        let env_filter = build_env_filter();

        let layer = tracing_subscriber::fmt::layer()
            .json()
            .with_timer(tracing_subscriber::fmt::time::UtcTime::rfc_3339())
            .with_writer(logger);

        let subscriber = tracing_subscriber::registry().with(env_filter).with(layer);
        let _ = subscriber.try_init();
    });
}

pub struct RunLogGuard {
    path: PathBuf,
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

pub fn start_run_log(kind: &str, run_id: &str, data_dir: &Path) -> std::io::Result<RunLogGuard> {
    init_run_logging();

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

    Ok(RunLogGuard { path })
}

fn resolve_log_dir(data_dir: &Path) -> PathBuf {
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
    fn env_filter_precedence_is_televybackup_then_rust_log_then_default() {
        let f1 = build_env_filter_from(Some("info"), Some("debug"));
        let f2 = build_env_filter_from(None, Some("warn"));
        let f3 = build_env_filter_from(None, None);

        assert_eq!(f1.to_string(), "info");
        assert_eq!(f2.to_string(), "warn");
        assert_eq!(f3.to_string(), "debug");
    }

    #[test]
    fn run_log_is_ndjson_and_flushed_on_drop() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let guard = start_run_log("backup", "tsk_test", temp.path()).expect("start_run_log");

        let expected_dir = temp.path().join("logs");
        assert_eq!(guard.path().parent(), Some(expected_dir.as_path()));

        tracing::info!(
            event = "run.start",
            kind = "backup",
            run_id = "tsk_test",
            task_id = "tsk_test",
            "run.start"
        );
        tracing::debug!(event = "phase.start", phase = "scan", "phase.start");
        tracing::debug!(event = "phase.finish", phase = "scan", "phase.finish");
        tracing::info!(
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
                fields.contains_key("message") || fields.contains_key("event"),
                "fields missing message/event"
            );
        }
    }
}
