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
    /// Best-effort wire bytes sent while uploading.
    ///
    /// This is not protocol payload accounting; it may exceed `bytes_uploaded` due to overhead,
    /// retries, and buffering. Intended for realtime rate indicators.
    pub net_bytes_uploaded: Option<u64>,
    pub bytes_downloaded: Option<u64>,
    /// Best-effort wire bytes received while downloading.
    ///
    /// This is not protocol payload accounting; it may exceed `bytes_downloaded` due to overhead,
    /// retries, and buffering. Intended for realtime rate indicators.
    pub net_bytes_downloaded: Option<u64>,
    pub bytes_deduped: Option<u64>,
}

pub trait ProgressSink: Send + Sync {
    fn on_progress(&self, progress: TaskProgress);
}
