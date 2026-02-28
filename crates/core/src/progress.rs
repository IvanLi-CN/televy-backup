use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TaskProgress {
    pub phase: String,
    pub files_total: Option<u64>,
    pub files_done: Option<u64>,
    pub source_files_total: Option<u64>,
    pub source_bytes_total: Option<u64>,
    /// Total source bytes that require upload in this run (dedup-excluded).
    pub source_bytes_need_upload_total: Option<u64>,
    pub chunks_total: Option<u64>,
    pub chunks_done: Option<u64>,
    pub bytes_read: Option<u64>,
    /// Total upload workload bytes discovered for this run (source payload + index payload).
    pub upload_bytes_total: Option<u64>,
    /// Confirmed persisted payload bytes for this run.
    pub bytes_uploaded_confirmed: Option<u64>,
    /// Uploaded source payload bytes only (excludes index/manifest uploads).
    pub bytes_uploaded_source: Option<u64>,
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
