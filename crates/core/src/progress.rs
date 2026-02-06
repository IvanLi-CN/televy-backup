use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskProgress {
    pub phase: String,
    pub files_total: Option<u64>,
    pub files_done: Option<u64>,
    pub chunks_total: Option<u64>,
    pub chunks_done: Option<u64>,
    pub bytes_read: Option<u64>,
    pub bytes_uploaded: Option<u64>,
    pub bytes_downloaded: Option<u64>,
    pub bytes_deduped: Option<u64>,
}

pub trait ProgressSink: Send + Sync {
    fn on_progress(&self, progress: TaskProgress);
}
