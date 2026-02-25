use std::collections::HashSet;
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fastcdc::ronomon::FastCDC;
use fastcdc::v2020::{ChunkData, Error as CdcError, MAXIMUM_MAX as V2020_MAXIMUM_MAX, StreamCDC};
use futures::stream::{FuturesUnordered, StreamExt};
use serde::{Deserialize, Serialize};
use sqlx::pool::PoolConnection;
use sqlx::{Connection, Row, Sqlite};
use tracing::{debug, error};
use walkdir::WalkDir;

use crate::config::TelegramRateLimit;
use crate::crypto::FRAMING_OVERHEAD_BYTES;
use crate::crypto::encrypt_framed;
use crate::index_db::open_index_db;
use crate::index_manifest::{IndexManifest, IndexManifestPart, index_part_aad};
use crate::pack::{
    PACK_MAX_BYTES, PACK_MAX_ENTRIES_PER_PACK, PACK_TARGET_BYTES, PACK_TARGET_JITTER_BYTES,
    PackBlob, PackBuilder,
};
use crate::progress::{ProgressSink, TaskProgress};
use crate::storage::MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES;
use crate::storage::{Storage, encode_tgfile_object_id, encode_tgpack_object_id};
use crate::{Error, Result};
use tokio::sync::{Mutex, Notify, OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::time::sleep;
use tokio_util::sync::CancellationToken;

const INDEX_PART_BYTES: usize = 4 * 1024 * 1024;
const PACK_ENABLE_MIN_OBJECTS: usize = 10;
const SINGLE_BLOB_PACK_OVERHEAD_BUDGET_BYTES: usize = 4096;
const RONOMON_READ_CHUNK_BYTES: usize = 1024 * 1024;
const PACK_MAX_STAGING_AGE_SECS: u64 = 3;
const BASE_FILE_CHUNK_COPY_BATCH_SIZE: usize = 128;
const ADAPTIVE_MIN_CONCURRENCY: usize = 1;
const ADAPTIVE_MAX_CONCURRENCY: usize = 8;
const ADAPTIVE_MAX_DELAY_MS: u64 = 500;
const ADAPTIVE_TICK_INTERVAL_SECS: u64 = 15;
const ADAPTIVE_WARMUP_SECS: u64 = 30;
const ADAPTIVE_UPGRADE_THROUGHPUT_BPS: u64 = 1024 * 1024;
const ADAPTIVE_UPGRADE_MAX_ERROR_RATE: f64 = 0.01;
const ADAPTIVE_DOWNGRADE_MIN_ERROR_RATE: f64 = 0.05;
const ADAPTIVE_CONSECUTIVE_FAILURES_DOWNGRADE: usize = 3;
const ADAPTIVE_UPSHIFT_DELAY_STEP_MS: i64 = -50;
const ADAPTIVE_DOWNSHIFT_DELAY_STEP_MS: i64 = 50;
const SQLITE_BUSY_RETRY_DELAYS_MS: [u64; 5] = [100, 250, 500, 1000, 2000];

type CdcResult<T> = std::result::Result<T, CdcError>;
type DbConn = PoolConnection<Sqlite>;

#[derive(Debug, Clone)]
pub struct ChunkingConfig {
    pub min_bytes: u32,
    pub avg_bytes: u32,
    pub max_bytes: u32,
}

impl ChunkingConfig {
    pub fn validate(&self) -> Result<()> {
        if self.min_bytes == 0 || self.avg_bytes == 0 || self.max_bytes == 0 {
            return Err(Error::InvalidConfig {
                message: "chunk sizes must be > 0".to_string(),
            });
        }
        if !(self.min_bytes <= self.avg_bytes && self.avg_bytes <= self.max_bytes) {
            return Err(Error::InvalidConfig {
                message: "chunk sizes must satisfy min <= avg <= max".to_string(),
            });
        }

        // Avoid panics from FastCDC internal assertions by validating bounds up-front.
        if self.max_bytes <= V2020_MAXIMUM_MAX {
            let min_ok = (fastcdc::v2020::MINIMUM_MIN..=fastcdc::v2020::MINIMUM_MAX)
                .contains(&self.min_bytes);
            let avg_ok = (fastcdc::v2020::AVERAGE_MIN..=fastcdc::v2020::AVERAGE_MAX)
                .contains(&self.avg_bytes);
            let max_ok = (fastcdc::v2020::MAXIMUM_MIN..=fastcdc::v2020::MAXIMUM_MAX)
                .contains(&self.max_bytes);
            if !(min_ok && avg_ok && max_ok) {
                return Err(Error::InvalidConfig {
                    message: format!(
                        "chunk sizes out of bounds for fastcdc::v2020 (min={}..={}, avg={}..={}, max>={})",
                        fastcdc::v2020::MINIMUM_MIN,
                        fastcdc::v2020::MINIMUM_MAX,
                        fastcdc::v2020::AVERAGE_MIN,
                        fastcdc::v2020::AVERAGE_MAX,
                        fastcdc::v2020::MAXIMUM_MIN,
                    ),
                });
            }
        } else {
            let min = self.min_bytes as usize;
            let avg = self.avg_bytes as usize;
            let max = self.max_bytes as usize;
            let min_ok =
                (fastcdc::ronomon::MINIMUM_MIN..=fastcdc::ronomon::MINIMUM_MAX).contains(&min);
            let avg_ok =
                (fastcdc::ronomon::AVERAGE_MIN..=fastcdc::ronomon::AVERAGE_MAX).contains(&avg);
            let max_ok =
                (fastcdc::ronomon::MAXIMUM_MIN..=fastcdc::ronomon::MAXIMUM_MAX).contains(&max);
            if !(min_ok && avg_ok && max_ok) {
                return Err(Error::InvalidConfig {
                    message: format!(
                        "chunk sizes out of bounds for fastcdc::ronomon (min={}..={}, avg={}..={}, max={}..={})",
                        fastcdc::ronomon::MINIMUM_MIN,
                        fastcdc::ronomon::MINIMUM_MAX,
                        fastcdc::ronomon::AVERAGE_MIN,
                        fastcdc::ronomon::AVERAGE_MAX,
                        fastcdc::ronomon::MAXIMUM_MIN,
                        fastcdc::ronomon::MAXIMUM_MAX,
                    ),
                });
            }
        }
        Ok(())
    }

    pub fn validate_for_provider(&self, provider: &str) -> Result<()> {
        self.validate()?;

        // MTProto-only: cap chunking.max_bytes to keep upload_document bytes <= engineered max.
        if provider.starts_with("telegram.mtproto") {
            let mtproto_max_plain_bytes =
                MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES.saturating_sub(FRAMING_OVERHEAD_BYTES);
            if self.max_bytes as usize > mtproto_max_plain_bytes {
                return Err(Error::InvalidConfig {
                    message: format!(
                        "chunking.max_bytes too large for MTProto storage: max_bytes={} must be <= {} (= MTProtoEngineeredUploadMaxBytes {} - framing_overhead {} bytes)",
                        self.max_bytes,
                        mtproto_max_plain_bytes,
                        MTPROTO_ENGINEERED_UPLOAD_MAX_BYTES,
                        FRAMING_OVERHEAD_BYTES,
                    ),
                });
            }
        }

        Ok(())
    }
}

fn file_chunker(
    file: File,
    chunking: &ChunkingConfig,
) -> Box<dyn Iterator<Item = CdcResult<ChunkData>>> {
    if chunking.max_bytes <= V2020_MAXIMUM_MAX {
        Box::new(StreamCDC::new(
            file,
            chunking.min_bytes,
            chunking.avg_bytes,
            chunking.max_bytes,
        ))
    } else {
        Box::new(RonomonStreamCDC::new(
            file,
            chunking.min_bytes as usize,
            chunking.avg_bytes as usize,
            chunking.max_bytes as usize,
        ))
    }
}

struct RonomonStreamCDC<R: Read> {
    source: R,
    buffer: Vec<u8>,
    eof: bool,
    processed: u64,
    min_size: usize,
    avg_size: usize,
    max_size: usize,
    buffer_start: usize,
}

impl<R: Read> RonomonStreamCDC<R> {
    fn new(source: R, min_size: usize, avg_size: usize, max_size: usize) -> Self {
        Self {
            source,
            buffer: Vec::with_capacity(std::cmp::min(max_size, RONOMON_READ_CHUNK_BYTES)),
            eof: false,
            processed: 0,
            min_size,
            avg_size,
            max_size,
            buffer_start: 0,
        }
    }

    fn available(&self) -> &[u8] {
        &self.buffer[self.buffer_start..]
    }

    fn compact_if_needed(&mut self) {
        if self.buffer_start == 0 {
            return;
        }
        if self.buffer_start >= self.buffer.len() {
            self.buffer.clear();
            self.buffer_start = 0;
            return;
        }
        if self.buffer_start < self.buffer.len() / 2 {
            return;
        }
        let start = self.buffer_start;
        self.buffer.copy_within(start.., 0);
        self.buffer.truncate(self.buffer.len() - start);
        self.buffer_start = 0;
    }

    fn read_more(&mut self) -> CdcResult<usize> {
        self.compact_if_needed();

        let mut tmp = vec![0u8; RONOMON_READ_CHUNK_BYTES];
        let n = self.source.read(&mut tmp).map_err(CdcError::IoError)?;
        if n == 0 {
            self.eof = true;
            return Ok(0);
        }
        tmp.truncate(n);
        self.buffer.extend_from_slice(&tmp);
        Ok(n)
    }
}

impl<R: Read> Iterator for RonomonStreamCDC<R> {
    type Item = CdcResult<ChunkData>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.available().is_empty() && self.eof {
                return None;
            }

            // Ensure enough bytes to find a cut point.
            while !self.eof && self.available().len() < self.max_size {
                match self.read_more() {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(e) => return Some(Err(e)),
                }
            }

            let available = self.available();
            if available.is_empty() && self.eof {
                return None;
            }

            let mut chunker = FastCDC::with_eof(
                available,
                self.min_size,
                self.avg_size,
                self.max_size,
                self.eof,
            );
            if let Some(chunk) = chunker.next() {
                let len = chunk.length;
                if len == 0 {
                    return Some(Err(CdcError::Other(
                        "chunking failed: zero-length chunk".to_string(),
                    )));
                }
                if len > available.len() {
                    return Some(Err(CdcError::Other(
                        "chunking failed: chunk length out of bounds".to_string(),
                    )));
                }

                let data = available[..len].to_vec();
                let out = ChunkData {
                    hash: chunk.hash as u64,
                    offset: self.processed,
                    length: len,
                    data,
                };

                self.buffer_start = self.buffer_start.saturating_add(len);
                self.processed = self.processed.saturating_add(len as u64);
                self.compact_if_needed();
                return Some(Ok(out));
            }

            if self.eof {
                return None;
            }
            match self.read_more() {
                Ok(0) => continue,
                Ok(_) => continue,
                Err(e) => return Some(Err(e)),
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackupConfig {
    pub db_path: PathBuf,
    pub source_path: PathBuf,
    pub label: String,
    pub chunking: ChunkingConfig,
    pub rate_limit: TelegramRateLimit,
    pub master_key: [u8; 32],
    pub snapshot_id: Option<String>,
    pub keep_last_snapshots: u32,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct BackupResult {
    pub snapshot_id: String,
    pub files_total: u64,
    pub files_indexed: u64,
    pub chunks_total: u64,
    pub chunks_uploaded: u64,
    pub data_objects_uploaded: u64,
    pub data_objects_estimated_without_pack: u64,
    pub bytes_read: u64,
    pub bytes_uploaded: u64,
    pub bytes_deduped: u64,
    pub index_parts: u64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct SourceQuickStats {
    pub files_total: u64,
    pub bytes_total: u64,
}

pub async fn run_backup<S: Storage>(storage: &S, config: BackupConfig) -> Result<BackupResult> {
    run_backup_with(storage, config, BackupOptions::default()).await
}

#[derive(Default)]
pub struct BackupOptions<'a> {
    pub cancel: Option<&'a CancellationToken>,
    pub progress: Option<&'a dyn ProgressSink>,
    pub source_quick_stats: Option<SourceQuickStats>,
}

#[derive(Debug, Clone)]
struct UploadLimits {
    worker_pool_size: usize,
    max_pending_jobs: usize,
    max_pending_bytes: usize,
}

fn compute_upload_limits(rate_limit: &TelegramRateLimit) -> Result<UploadLimits> {
    if rate_limit.max_concurrent_uploads < 1 {
        return Err(Error::InvalidConfig {
            message: "telegram_endpoints[].rate_limit.max_concurrent_uploads must be >= 1"
                .to_string(),
        });
    }
    let configured_concurrency = rate_limit.max_concurrent_uploads as usize;
    if configured_concurrency > ADAPTIVE_MAX_CONCURRENCY {
        return Err(Error::InvalidConfig {
            message: format!(
                "telegram_endpoints[].rate_limit.max_concurrent_uploads must be <= {ADAPTIVE_MAX_CONCURRENCY} for adaptive mode"
            ),
        });
    }
    // Keep enough workers ready for adaptive upshifts even if config starts low.
    let worker_pool_size = ADAPTIVE_MAX_CONCURRENCY;
    let max_pending_jobs = worker_pool_size.saturating_mul(8).max(1);
    let max_pending_bytes = worker_pool_size
        .saturating_mul(PACK_MAX_BYTES)
        .saturating_mul(2);
    Ok(UploadLimits {
        worker_pool_size,
        max_pending_jobs,
        max_pending_bytes,
    })
}

pub fn compute_source_quick_stats(
    source_path: &Path,
    cancel: Option<&CancellationToken>,
) -> Result<SourceQuickStats> {
    let mut files_total = 0u64;
    let mut bytes_total = 0u64;

    for entry in WalkDir::new(source_path).follow_links(false) {
        if let Some(cancel) = cancel
            && cancel.is_cancelled()
        {
            return Err(Error::Cancelled);
        }

        let entry = match entry {
            Ok(v) => v,
            Err(e) => {
                let is_not_found = e
                    .io_error()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                let is_root = e.path().is_some_and(|p| p == source_path);
                if is_not_found && !is_root {
                    continue;
                }
                return Err(Error::Walkdir(e));
            }
        };

        let path = entry.path();
        if path == source_path {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(v) => v,
            Err(e) => {
                let is_not_found = e
                    .io_error()
                    .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                if is_not_found {
                    continue;
                }
                return Err(Error::Walkdir(e));
            }
        };

        if !metadata.is_file() {
            continue;
        }

        files_total = files_total.saturating_add(1);
        bytes_total = bytes_total.saturating_add(metadata.len());
    }

    Ok(SourceQuickStats {
        files_total,
        bytes_total,
    })
}

#[derive(Debug)]
struct UploadRateLimiter {
    min_delay_floor_ms: u64,
    max_delay_ms: u64,
    min_delay_ms: AtomicU64,
    next_allowed: Mutex<Instant>,
}

impl UploadRateLimiter {
    fn new(initial_delay_ms: u64, min_delay_floor_ms: u64, max_delay_ms: u64) -> Self {
        let min_delay_floor_ms = min_delay_floor_ms.min(max_delay_ms);
        Self {
            min_delay_floor_ms,
            max_delay_ms,
            min_delay_ms: AtomicU64::new(initial_delay_ms.clamp(min_delay_floor_ms, max_delay_ms)),
            next_allowed: Mutex::new(Instant::now()),
        }
    }

    fn min_delay_ms(&self) -> u64 {
        self.min_delay_ms.load(Ordering::Relaxed)
    }

    fn adjust_min_delay_ms(&self, delta_ms: i64) -> (u64, u64) {
        loop {
            let current = self.min_delay_ms.load(Ordering::Relaxed);
            let adjusted = if delta_ms >= 0 {
                current.saturating_add(delta_ms as u64)
            } else {
                current.saturating_sub(delta_ms.unsigned_abs())
            }
            .clamp(self.min_delay_floor_ms, self.max_delay_ms);
            if self
                .min_delay_ms
                .compare_exchange(current, adjusted, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return (current, adjusted);
            }
        }
    }

    async fn wait_turn(&self) {
        let min_delay_ms = self.min_delay_ms();
        if min_delay_ms == 0 {
            return;
        }
        let min_delay = Duration::from_millis(min_delay_ms);
        let now = Instant::now();
        let scheduled = {
            let mut guard = self.next_allowed.lock().await;
            let scheduled = if *guard > now { *guard } else { now };
            *guard = scheduled + min_delay;
            scheduled
        };
        if scheduled > now {
            sleep(scheduled - now).await;
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct AdaptiveWindowMetrics {
    attempts: u64,
    failures: u64,
    consecutive_failures: usize,
}

#[derive(Debug, Clone, Copy)]
struct AdaptiveShiftResult {
    changed: bool,
    previous_concurrency: usize,
    current_concurrency: usize,
    previous_delay_ms: u64,
    current_delay_ms: u64,
}

#[derive(Debug)]
struct AdaptiveUploadController {
    min_concurrency: usize,
    max_concurrency: usize,
    target_concurrency: AtomicUsize,
    slots_in_use: AtomicUsize,
    window_attempts: AtomicU64,
    window_failures: AtomicU64,
    consecutive_failures: AtomicUsize,
    limiter: Arc<UploadRateLimiter>,
    notify: Notify,
}

struct AdaptiveUploadSlot {
    controller: Arc<AdaptiveUploadController>,
}

impl Drop for AdaptiveUploadSlot {
    fn drop(&mut self) {
        saturating_sub_usize(&self.controller.slots_in_use, 1);
        self.controller.notify.notify_waiters();
    }
}

impl AdaptiveUploadController {
    fn new(
        initial_concurrency: usize,
        min_concurrency: usize,
        max_concurrency: usize,
        limiter: Arc<UploadRateLimiter>,
    ) -> Self {
        let min_concurrency = min_concurrency.max(ADAPTIVE_MIN_CONCURRENCY);
        let max_concurrency = max_concurrency.max(min_concurrency);
        Self {
            min_concurrency,
            max_concurrency,
            target_concurrency: AtomicUsize::new(
                initial_concurrency.clamp(min_concurrency, max_concurrency),
            ),
            slots_in_use: AtomicUsize::new(0),
            window_attempts: AtomicU64::new(0),
            window_failures: AtomicU64::new(0),
            consecutive_failures: AtomicUsize::new(0),
            limiter,
            notify: Notify::new(),
        }
    }

    fn target_concurrency(&self) -> usize {
        self.target_concurrency.load(Ordering::Relaxed)
    }

    fn min_delay_ms(&self) -> u64 {
        self.limiter.min_delay_ms()
    }

    async fn acquire_slot(
        self: &Arc<Self>,
        cancel: &CancellationToken,
    ) -> Result<AdaptiveUploadSlot> {
        loop {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }

            let target = self.target_concurrency();
            let in_use = self.slots_in_use.load(Ordering::Relaxed);
            if in_use < target {
                if self
                    .slots_in_use
                    .compare_exchange(in_use, in_use + 1, Ordering::Relaxed, Ordering::Relaxed)
                    .is_ok()
                {
                    return Ok(AdaptiveUploadSlot {
                        controller: Arc::clone(self),
                    });
                }
                continue;
            }

            tokio::select! {
                _ = self.notify.notified() => {},
                _ = cancel.cancelled() => return Err(Error::Cancelled),
            }
        }
    }

    fn on_attempt(&self) {
        self.window_attempts.fetch_add(1, Ordering::Relaxed);
    }

    fn on_success(&self) {
        self.consecutive_failures.store(0, Ordering::Relaxed);
    }

    fn on_failure(&self, error: &Error) -> Option<AdaptiveShiftResult> {
        self.window_failures.fetch_add(1, Ordering::Relaxed);
        self.consecutive_failures.fetch_add(1, Ordering::Relaxed);
        if !error_has_flood_wait(error) {
            return None;
        }
        let shift = self.try_shift_down(ADAPTIVE_DOWNSHIFT_DELAY_STEP_MS);
        if shift.changed {
            debug!(
                event = "upload.adaptive.tick",
                action = "downshift_flood_wait",
                target_concurrency = shift.current_concurrency,
                previous_concurrency = shift.previous_concurrency,
                min_delay_ms = shift.current_delay_ms,
                previous_delay_ms = shift.previous_delay_ms,
                "upload.adaptive.tick"
            );
            return Some(shift);
        }
        None
    }

    fn take_window_metrics(&self) -> AdaptiveWindowMetrics {
        AdaptiveWindowMetrics {
            attempts: self.window_attempts.swap(0, Ordering::Relaxed),
            failures: self.window_failures.swap(0, Ordering::Relaxed),
            consecutive_failures: self.consecutive_failures.load(Ordering::Relaxed),
        }
    }

    fn try_shift_up(&self) -> AdaptiveShiftResult {
        self.try_shift(1, ADAPTIVE_UPSHIFT_DELAY_STEP_MS)
    }

    fn try_shift_down(&self, delay_step_ms: i64) -> AdaptiveShiftResult {
        self.try_shift(-1, delay_step_ms)
    }

    fn try_shift(&self, concurrency_delta: i32, delay_delta_ms: i64) -> AdaptiveShiftResult {
        let (previous_concurrency, current_concurrency) =
            self.adjust_target_concurrency(concurrency_delta);
        let (previous_delay_ms, current_delay_ms) =
            self.limiter.adjust_min_delay_ms(delay_delta_ms);
        let changed =
            previous_concurrency != current_concurrency || previous_delay_ms != current_delay_ms;
        if changed {
            self.notify.notify_waiters();
        }
        AdaptiveShiftResult {
            changed,
            previous_concurrency,
            current_concurrency,
            previous_delay_ms,
            current_delay_ms,
        }
    }

    fn adjust_target_concurrency(&self, delta: i32) -> (usize, usize) {
        loop {
            let current = self.target_concurrency();
            let next = (current as i32 + delta)
                .clamp(self.min_concurrency as i32, self.max_concurrency as i32)
                as usize;
            if self
                .target_concurrency
                .compare_exchange(current, next, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
            {
                return (current, next);
            }
        }
    }
}

fn error_has_flood_wait(error: &Error) -> bool {
    match error {
        Error::Telegram { message } => message.to_ascii_uppercase().contains("FLOOD_WAIT"),
        _ => false,
    }
}

fn saturating_sub_usize(atom: &AtomicUsize, delta: usize) {
    let _ = atom.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(delta))
    });
}

fn saturating_sub_u64(atom: &AtomicU64, delta: u64) {
    let _ = atom.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
        Some(current.saturating_sub(delta))
    });
}

fn is_sqlite_busy_or_locked(error: &sqlx::Error) -> bool {
    if let sqlx::Error::Database(db_error) = error
        && db_error
            .code()
            .as_deref()
            .is_some_and(|code| matches!(code, "5" | "6" | "SQLITE_BUSY" | "SQLITE_LOCKED"))
    {
        return true;
    }
    let msg = error.to_string().to_ascii_lowercase();
    msg.contains("database is locked")
        || msg.contains("database table is locked")
        || msg.contains("database is busy")
        || msg.contains("sqlite_busy")
        || msg.contains("sqlite_locked")
}

macro_rules! execute_sqlite_with_busy_retry {
    ($op_name:expr, $query:expr) => {{
        let mut retry_idx = 0usize;
        loop {
            match $query.await {
                Ok(v) => break Ok(v),
                Err(e)
                    if is_sqlite_busy_or_locked(&e)
                        && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len() =>
                {
                    let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                    retry_idx += 1;
                    debug!(
                        event = "sqlite.busy_retry",
                        operation = $op_name,
                        retry = retry_idx,
                        wait_ms,
                        error = %e,
                        "sqlite.busy_retry"
                    );
                    sleep(Duration::from_millis(wait_ms)).await;
                }
                Err(e) => break Err(Error::Sqlite(e)),
            }
        }
    }};
}

#[derive(Debug, Clone)]
struct PackEntryRef {
    chunk_hash: String,
    offset: u64,
    len: u64,
}

#[derive(Debug)]
enum UploadJob {
    Direct {
        chunk_hash: String,
        blob: Vec<u8>,
        _bytes_permit: OwnedSemaphorePermit,
    },
    Pack {
        entries: Vec<PackEntryRef>,
        pack_bytes: Vec<u8>,
        _bytes_permit: OwnedSemaphorePermit,
    },
}

impl UploadJob {
    fn payload_len(&self) -> usize {
        match self {
            UploadJob::Direct { blob, .. } => blob.len(),
            UploadJob::Pack { pack_bytes, .. } => pack_bytes.len(),
        }
    }
}

#[derive(Debug)]
enum UploadOutcome {
    Direct {
        chunk_hash: String,
        object_id: String,
        bytes: u64,
    },
    Pack {
        entries: Vec<PackEntryRef>,
        pack_object_id: String,
        bytes: u64,
    },
}

#[derive(Debug, Clone)]
struct ChunkObjectMapping {
    chunk_hash: String,
    object_id: String,
}

#[derive(Debug, Clone)]
struct FileChunkRow {
    seq: i64,
    chunk_hash: String,
    offset: i64,
    len: i64,
}

#[derive(Debug, Clone)]
struct BaseFileSnapshotRow {
    file_id: String,
    size: i64,
    mtime_ms: i64,
    mode: i64,
}

#[derive(Debug, Clone)]
struct BaseFileChunkCopyRow {
    file_id: String,
    base_file_id: String,
    size: u64,
}

#[derive(Clone)]
struct UploadQueue {
    sender: mpsc::Sender<UploadJob>,
    bytes_sem: Arc<Semaphore>,
    bytes_budget: usize,
    pending_jobs: Arc<AtomicUsize>,
    pending_bytes: Arc<AtomicU64>,
    cancel: CancellationToken,
}

impl UploadQueue {
    async fn enqueue_direct(&self, chunk_hash: String, blob: Vec<u8>) -> Result<()> {
        let bytes = blob.len();
        let permit = acquire_bytes(&self.bytes_sem, self.bytes_budget, bytes, &self.cancel).await?;
        self.pending_jobs.fetch_add(1, Ordering::Relaxed);
        self.pending_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        let job = UploadJob::Direct {
            chunk_hash,
            blob,
            _bytes_permit: permit,
        };
        if self.sender.send(job).await.is_err() {
            saturating_sub_usize(self.pending_jobs.as_ref(), 1);
            saturating_sub_u64(self.pending_bytes.as_ref(), bytes as u64);
            return Err(Error::Telegram {
                message: "upload queue closed".to_string(),
            });
        }
        Ok(())
    }

    async fn enqueue_pack(&self, entries: Vec<PackEntryRef>, pack_bytes: Vec<u8>) -> Result<()> {
        let bytes = pack_bytes.len();
        let permit = acquire_bytes(&self.bytes_sem, self.bytes_budget, bytes, &self.cancel).await?;
        self.pending_jobs.fetch_add(1, Ordering::Relaxed);
        self.pending_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        let job = UploadJob::Pack {
            entries,
            pack_bytes,
            _bytes_permit: permit,
        };
        if self.sender.send(job).await.is_err() {
            saturating_sub_usize(self.pending_jobs.as_ref(), 1);
            saturating_sub_u64(self.pending_bytes.as_ref(), bytes as u64);
            return Err(Error::Telegram {
                message: "upload queue closed".to_string(),
            });
        }
        Ok(())
    }
}

async fn acquire_bytes(
    bytes_sem: &Arc<Semaphore>,
    bytes_budget: usize,
    bytes: usize,
    cancel: &CancellationToken,
) -> Result<OwnedSemaphorePermit> {
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    if bytes > bytes_budget {
        return Err(Error::InvalidConfig {
            message: format!(
                "upload bytes {bytes} exceeds queue budget {bytes_budget}; adjust rate_limit or chunking"
            ),
        });
    }
    let bytes_u32 = u32::try_from(bytes).map_err(|_| Error::InvalidConfig {
        message: format!("upload bytes too large: {bytes}"),
    })?;
    tokio::select! {
        permit = bytes_sem.clone().acquire_many_owned(bytes_u32) => {
            permit.map_err(|_| Error::Telegram {
                message: "upload queue closed".to_string(),
            })
        }
        _ = cancel.cancelled() => Err(Error::Cancelled),
    }
}

async fn process_upload_job<S: Storage>(
    storage: &S,
    provider: &str,
    limiter: &UploadRateLimiter,
    uploaded_bytes: &AtomicU64,
    uploaded_net_bytes: &AtomicU64,
    have_uploaded_net_bytes: &AtomicBool,
    job: UploadJob,
) -> Result<UploadOutcome> {
    match job {
        UploadJob::Direct {
            chunk_hash,
            blob,
            _bytes_permit,
        } => {
            let bytes_len = blob.len() as u64;
            limiter.wait_turn().await;
            let filename = telegram_camouflaged_filename();
            let last_reported = Arc::new(AtomicU64::new(0));
            let last_reported_net = Arc::new(AtomicU64::new(0));
            let last_for_cb = Arc::clone(&last_reported);
            let last_net_for_cb = Arc::clone(&last_reported_net);
            let object_id = storage
                .upload_document_with_progress(
                    &filename,
                    blob,
                    Some(Box::new(move |p| {
                        let n = p.bytes;
                        let prev = last_for_cb.swap(n, Ordering::Relaxed);
                        if n > prev {
                            uploaded_bytes.fetch_add(n - prev, Ordering::Relaxed);
                        }

                        if let Some(net) = p.net_bytes {
                            have_uploaded_net_bytes.store(true, Ordering::Relaxed);
                            let prev_net = last_net_for_cb.swap(net, Ordering::Relaxed);
                            if net > prev_net {
                                uploaded_net_bytes.fetch_add(net - prev_net, Ordering::Relaxed);
                            }
                        }
                    })),
                )
                .await
                .map_err(|e| {
                    error!(
                        event = "io.telegram.upload_failed",
                        provider,
                        chunk_hash,
                        blob_bytes = bytes_len,
                        error = %e,
                        "io.telegram.upload_failed"
                    );
                    Error::Telegram {
                        message: format!(
                            "upload failed: kind=direct chunk_hash={chunk_hash} bytes={bytes_len}; {e}"
                        ),
                    }
                })?;
            let reported = last_reported.load(Ordering::Relaxed);
            if reported < bytes_len {
                uploaded_bytes.fetch_add(bytes_len - reported, Ordering::Relaxed);
            }
            Ok(UploadOutcome::Direct {
                chunk_hash,
                object_id,
                bytes: bytes_len,
            })
        }
        UploadJob::Pack {
            entries,
            pack_bytes,
            _bytes_permit,
        } => {
            let bytes_len = pack_bytes.len() as u64;
            limiter.wait_turn().await;
            let filename = telegram_camouflaged_filename();
            let last_reported = Arc::new(AtomicU64::new(0));
            let last_reported_net = Arc::new(AtomicU64::new(0));
            let last_for_cb = Arc::clone(&last_reported);
            let last_net_for_cb = Arc::clone(&last_reported_net);
            let pack_object_id = storage
                .upload_document_with_progress(
                    &filename,
                    pack_bytes,
                    Some(Box::new(move |p| {
                        let n = p.bytes;
                        let prev = last_for_cb.swap(n, Ordering::Relaxed);
                        if n > prev {
                            uploaded_bytes.fetch_add(n - prev, Ordering::Relaxed);
                        }

                        if let Some(net) = p.net_bytes {
                            have_uploaded_net_bytes.store(true, Ordering::Relaxed);
                            let prev_net = last_net_for_cb.swap(net, Ordering::Relaxed);
                            if net > prev_net {
                                uploaded_net_bytes.fetch_add(net - prev_net, Ordering::Relaxed);
                            }
                        }
                    })),
                )
                .await
                .map_err(|e| {
                    error!(
                        event = "io.telegram.upload_failed",
                        provider,
                        blob_bytes = bytes_len,
                        error = %e,
                        "io.telegram.upload_failed"
                    );
                    Error::Telegram {
                        message: format!("upload failed: kind=pack bytes={bytes_len}; {e}"),
                    }
                })?;
            let reported = last_reported.load(Ordering::Relaxed);
            if reported < bytes_len {
                uploaded_bytes.fetch_add(bytes_len - reported, Ordering::Relaxed);
            }
            Ok(UploadOutcome::Pack {
                entries,
                pack_object_id,
                bytes: bytes_len,
            })
        }
    }
}

pub async fn run_backup_with<S: Storage>(
    storage: &S,
    config: BackupConfig,
    options: BackupOptions<'_>,
) -> Result<BackupResult> {
    debug!(
        event = "backup.prepare",
        db_path = %config.db_path.display(),
        source_path = %config.source_path.display(),
        label = %config.label,
        keep_last_snapshots = config.keep_last_snapshots,
        "backup.prepare"
    );
    let scan_started = Instant::now();
    debug!(event = "phase.start", phase = "scan", "phase.start");

    let provider = storage.provider();
    config.chunking.validate_for_provider(provider)?;
    if config.keep_last_snapshots < 1 {
        return Err(Error::InvalidConfig {
            message: "keep_last_snapshots must be >= 1".to_string(),
        });
    }
    if !config.source_path.is_dir() {
        return Err(Error::InvalidConfig {
            message: "source_path must be an existing directory".to_string(),
        });
    }

    let source_quick_stats = options.source_quick_stats;
    let source_files_total = source_quick_stats.map(|s| s.files_total);
    let source_bytes_total = source_quick_stats.map(|s| s.bytes_total);

    let provider_owned = provider.to_string();
    let limits = compute_upload_limits(&config.rate_limit)?;
    let configured_concurrency = config.rate_limit.max_concurrent_uploads as usize;
    let adaptive_max_concurrency = ADAPTIVE_MAX_CONCURRENCY;
    let initial_concurrency = configured_concurrency;
    let configured_delay_ms = config.rate_limit.min_delay_ms as u64;
    if configured_delay_ms > ADAPTIVE_MAX_DELAY_MS {
        return Err(Error::InvalidConfig {
            message: format!(
                "telegram_endpoints[].rate_limit.min_delay_ms must be <= {ADAPTIVE_MAX_DELAY_MS} for adaptive mode"
            ),
        });
    }
    let adaptive_min_delay_ms = 0;
    let initial_delay_ms = configured_delay_ms;
    let rate_limiter = Arc::new(UploadRateLimiter::new(
        initial_delay_ms,
        adaptive_min_delay_ms,
        ADAPTIVE_MAX_DELAY_MS,
    ));
    let adaptive_controller = Arc::new(AdaptiveUploadController::new(
        initial_concurrency,
        ADAPTIVE_MIN_CONCURRENCY,
        adaptive_max_concurrency,
        Arc::clone(&rate_limiter),
    ));

    let scan_files_indexed = Arc::new(AtomicU64::new(0));
    let scan_chunks_total = Arc::new(AtomicU64::new(0));
    let scan_bytes_read = Arc::new(AtomicU64::new(0));
    let scan_bytes_deduped = Arc::new(AtomicU64::new(0));
    let uploaded_bytes = Arc::new(AtomicU64::new(0));
    let uploaded_net_bytes = Arc::new(AtomicU64::new(0));
    let have_uploaded_net_bytes = Arc::new(AtomicBool::new(false));
    let scan_done = Arc::new(AtomicBool::new(false));
    let active_uploads = Arc::new(AtomicUsize::new(0));
    let pending_jobs = Arc::new(AtomicUsize::new(0));
    let pending_bytes = Arc::new(AtomicU64::new(0));

    let scan_source_path = config.source_path.clone();
    let scan_snapshot_id = config.snapshot_id.clone();
    let scan_label = config.label.clone();
    let scan_chunking = config.chunking.clone();
    let scan_master_key = config.master_key;

    let bytes_budget = u32::try_from(limits.max_pending_bytes).unwrap_or(u32::MAX) as usize;
    let upload_cancel = options
        .cancel
        .map(CancellationToken::child_token)
        .unwrap_or_default();
    let (upload_tx, upload_rx) = mpsc::channel::<UploadJob>(limits.max_pending_jobs);
    let (result_tx, result_rx) = mpsc::channel::<Result<UploadOutcome>>(limits.max_pending_jobs);
    let bytes_sem = Arc::new(Semaphore::new(bytes_budget));
    let uploader = UploadQueue {
        sender: upload_tx.clone(),
        bytes_sem: bytes_sem.clone(),
        bytes_budget,
        pending_jobs: Arc::clone(&pending_jobs),
        pending_bytes: Arc::clone(&pending_bytes),
        cancel: upload_cancel.clone(),
    };

    let upload_rx = Arc::new(Mutex::new(upload_rx));
    let mut workers = FuturesUnordered::new();
    for _ in 0..limits.worker_pool_size {
        let rx = Arc::clone(&upload_rx);
        let tx = result_tx.clone();
        let limiter = rate_limiter.clone();
        let adaptive = Arc::clone(&adaptive_controller);
        let provider = provider_owned.clone();
        let cancel = upload_cancel.clone();
        let uploaded_bytes = Arc::clone(&uploaded_bytes);
        let uploaded_net_bytes = Arc::clone(&uploaded_net_bytes);
        let have_uploaded_net_bytes = Arc::clone(&have_uploaded_net_bytes);
        let active_uploads = Arc::clone(&active_uploads);
        let pending_jobs = Arc::clone(&pending_jobs);
        let pending_bytes = Arc::clone(&pending_bytes);
        workers.push(async move {
            struct ActiveUploadToken<'a>(&'a AtomicUsize);
            impl Drop for ActiveUploadToken<'_> {
                fn drop(&mut self) {
                    self.0.fetch_sub(1, Ordering::Relaxed);
                }
            }

            loop {
                let slot = tokio::select! {
                    _ = cancel.cancelled() => break,
                    slot = adaptive.acquire_slot(&cancel) => slot,
                };
                let _slot = match slot {
                    Ok(v) => v,
                    Err(Error::Cancelled) => break,
                    Err(e) => {
                        let _ = tx.send(Err(e)).await;
                        break;
                    }
                };

                let job = tokio::select! {
                    _ = cancel.cancelled() => break,
                    job = async {
                        let mut guard = rx.lock().await;
                        guard.recv().await
                    } => job,
                };
                let Some(job) = job else {
                    break;
                };
                saturating_sub_usize(pending_jobs.as_ref(), 1);
                saturating_sub_u64(pending_bytes.as_ref(), job.payload_len() as u64);
                if cancel.is_cancelled() {
                    break;
                }
                active_uploads.fetch_add(1, Ordering::Relaxed);
                let _token = ActiveUploadToken(active_uploads.as_ref());
                adaptive.on_attempt();
                let outcome = process_upload_job(
                    storage,
                    &provider,
                    &limiter,
                    uploaded_bytes.as_ref(),
                    uploaded_net_bytes.as_ref(),
                    have_uploaded_net_bytes.as_ref(),
                    job,
                )
                .await;
                match &outcome {
                    Ok(_) => adaptive.on_success(),
                    Err(e) => {
                        let _ = adaptive.on_failure(e);
                    }
                }
                if tx.send(outcome).await.is_err() {
                    break;
                }
            }
        });
    }
    drop(result_tx);

    // Acquire a single dedicated SQLite connection for the entire backup pipeline. This avoids
    // pool acquisition stalls/timeouts under heavy scan+upload workloads (especially for large
    // `file_chunks` tables).
    let pool = open_index_db(&config.db_path).await?;
    let mut conn: DbConn = pool.acquire().await?;
    drop(pool);

    let scan_future = {
        let conn = &mut conn;
        let uploader = uploader.clone();
        let upload_tx = upload_tx.clone();
        let upload_tx_for_error = upload_tx.clone();
        let cancel = upload_cancel.clone();
        let scan_files_indexed = Arc::clone(&scan_files_indexed);
        let scan_chunks_total = Arc::clone(&scan_chunks_total);
        let scan_bytes_read = Arc::clone(&scan_bytes_read);
        let scan_bytes_deduped = Arc::clone(&scan_bytes_deduped);
        let uploaded_bytes = Arc::clone(&uploaded_bytes);
        let uploaded_net_bytes = Arc::clone(&uploaded_net_bytes);
        let have_uploaded_net_bytes = Arc::clone(&have_uploaded_net_bytes);
        let scan_done = Arc::clone(&scan_done);
        async move {
            let res = async {
                let base_snapshot_id =
                    latest_snapshot_for_source(conn, &scan_source_path).await?;
                let snapshot_id = scan_snapshot_id
                    .clone()
                    .unwrap_or_else(|| format!("snp_{}", uuid::Uuid::new_v4()));
                let source_path_utf8 = path_to_utf8(&scan_source_path)?;

                execute_sqlite_with_busy_retry!(
                    "snapshots.insert",
                    sqlx::query(
                        r#"
                        INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id)
                        VALUES (?, strftime('%Y-%m-%dT%H:%M:%fZ','now'), ?, ?, ?)
                        "#,
                    )
                    .bind(&snapshot_id)
                    .bind(&source_path_utf8)
                    .bind(&scan_label)
                    .bind(&base_snapshot_id)
                    .execute(&mut **conn)
                )?;

                let mut result = BackupResult {
                    snapshot_id: snapshot_id.clone(),
                    ..BackupResult::default()
                };

                let mut known_chunk_hashes =
                    load_chunk_hashes_for_storage(conn, storage, provider).await?;
                let mut pack_enabled = false;
                let mut pending_bytes: usize = 0;
                let mut pending: Vec<PackBlob> = Vec::new();
                let mut pending_base_chunk_copies: Vec<BaseFileChunkCopyRow> = Vec::new();
                let mut pack_state = PackState::new(provider, &snapshot_id);

                if let Some(sink) = options.progress {
                    sink.on_progress(TaskProgress {
                        phase: "scan".to_string(),
                        files_total: None,
                        files_done: Some(0),
                        source_files_total,
                        source_bytes_total,
                        chunks_total: Some(0),
                        chunks_done: Some(0),
                        bytes_read: Some(0),
                        bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
                        net_bytes_uploaded: have_uploaded_net_bytes
                            .load(Ordering::Relaxed)
                            .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
                        bytes_downloaded: None,
                        net_bytes_downloaded: None,
                        bytes_deduped: Some(0),
                    });
                }

                for entry in WalkDir::new(&scan_source_path).follow_links(false) {
                    if let Some(cancel) = options.cancel
                        && cancel.is_cancelled()
                    {
                        return Err(Error::Cancelled);
                    }

                    let entry = match entry {
                        Ok(v) => v,
                        Err(e) => {
                            let is_not_found = e
                                .io_error()
                                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                            let is_root = e.path().is_some_and(|p| p == scan_source_path);
                            if is_not_found && !is_root {
                                debug!(
                                    event = "scan.walkdir.not_found",
                                    path = %e.path().unwrap_or(Path::new("")).display(),
                                    error = %e,
                                    "scan.walkdir.not_found"
                                );
                                continue;
                            }
                            return Err(Error::Walkdir(e));
                        }
                    };

                    let path = entry.path();
                    if path == scan_source_path {
                        continue;
                    }

                    let rel_path =
                        path.strip_prefix(&scan_source_path)
                            .map_err(|_| Error::InvalidConfig {
                                message: "path strip_prefix failed".to_string(),
                            })?;
                    let rel_path_str = path_to_utf8(rel_path)?;

                    let metadata = match entry.metadata() {
                        Ok(v) => v,
                        Err(e) => {
                            let is_not_found = e
                                .io_error()
                                .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound);
                            if is_not_found {
                                debug!(
                                    event = "scan.entry_not_found",
                                    path = %path.display(),
                                    error = %e,
                                    "scan.entry_not_found"
                                );
                                continue;
                            }
                            return Err(Error::Walkdir(e));
                        }
                    };

                    let kind = if metadata.is_dir() {
                        "dir"
                    } else if metadata.is_file() {
                        "file"
                    } else if metadata.is_symlink() {
                        "symlink"
                    } else {
                        continue;
                    };

                    let (size, mtime_ms, mode) = if kind == "file" {
                        let size = metadata.len() as i64;
                        let mtime_ms = metadata
                            .modified()
                            .ok()
                            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                            .map(|d| d.as_millis() as i64)
                            .unwrap_or(0);
                        #[cfg(unix)]
                        let mode = {
                            use std::os::unix::fs::MetadataExt;
                            metadata.mode() as i64
                        };
                        #[cfg(not(unix))]
                        let mode = 0i64;
                        (size, mtime_ms, mode)
                    } else {
                        (0i64, 0i64, 0i64)
                    };

                    result.files_total += 1;

                    let file_id = format!("f_{}", uuid::Uuid::new_v4());
                    execute_sqlite_with_busy_retry!(
                        "files.insert",
                        sqlx::query(
                            r#"
                            INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind)
                            VALUES (?, ?, ?, ?, ?, ?, ?)
                            "#,
                        )
                        .bind(&file_id)
                        .bind(&snapshot_id)
                        .bind(&rel_path_str)
                        .bind(size)
                        .bind(mtime_ms)
                        .bind(mode)
                        .bind(kind)
                        .execute(&mut **conn)
                    )?;

                    result.files_indexed += 1;
                    scan_files_indexed.store(result.files_indexed, Ordering::Relaxed);

                    if kind != "file" {
                        continue;
                    }

                    if let Some(base_snapshot_id) = base_snapshot_id.as_deref()
                        && let Some(base_row) =
                            lookup_base_file_snapshot_row(conn, base_snapshot_id, &rel_path_str)
                                .await?
                        && base_row.size == size
                        && base_row.mtime_ms == mtime_ms
                        && base_row.mode == mode
                    {
                        pending_base_chunk_copies.push(BaseFileChunkCopyRow {
                            file_id: file_id.clone(),
                            base_file_id: base_row.file_id,
                            size: size.max(0) as u64,
                        });
                        if pending_base_chunk_copies.len() >= BASE_FILE_CHUNK_COPY_BATCH_SIZE {
                            let (copied_chunks, deduped_bytes) =
                                flush_base_chunk_copy_batch(conn, &mut pending_base_chunk_copies)
                                    .await?;
                            if copied_chunks > 0 {
                                result.chunks_total =
                                    result.chunks_total.saturating_add(copied_chunks);
                                scan_chunks_total.store(result.chunks_total, Ordering::Relaxed);
                            }
                            if deduped_bytes > 0 {
                                result.bytes_deduped =
                                    result.bytes_deduped.saturating_add(deduped_bytes);
                                scan_bytes_deduped.store(result.bytes_deduped, Ordering::Relaxed);
                            }
                        }
                        continue;
                    }

                    let file = match File::open(path) {
                        Ok(f) => f,
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            debug!(
                                event = "scan.file_not_found",
                                path = %path.display(),
                                error = %e,
                                "scan.file_not_found"
                            );
                            continue;
                        }
                        Err(e) => return Err(e.into()),
                    };
                    let chunker = file_chunker(file, &scan_chunking);
                    let mut file_chunk_rows: Vec<FileChunkRow> = Vec::new();

                    for (seq, chunk) in chunker.enumerate() {
                        if let Some(cancel) = options.cancel
                            && cancel.is_cancelled()
                        {
                            return Err(Error::Cancelled);
                        }

                        let chunk = chunk.map_err(|_| Error::InvalidConfig {
                            message: "chunking failed".to_string(),
                        })?;
                        result.chunks_total += 1;
                        scan_chunks_total.store(result.chunks_total, Ordering::Relaxed);
                        result.bytes_read += chunk.data.len() as u64;
                        scan_bytes_read.store(result.bytes_read, Ordering::Relaxed);

                        let chunk_hash = blake3::hash(&chunk.data).to_hex().to_string();

                        let exists = known_chunk_hashes.contains(&chunk_hash);
                        if exists {
                            result.bytes_deduped += chunk.data.len() as u64;
                            scan_bytes_deduped.store(result.bytes_deduped, Ordering::Relaxed);
                        } else {
                            known_chunk_hashes.insert(chunk_hash.clone());

                            let encrypted = encrypt_framed(
                                &scan_master_key,
                                chunk_hash.as_bytes(),
                                &chunk.data,
                            )?;

                            execute_sqlite_with_busy_retry!(
                                "chunks.insert",
                                sqlx::query(
                                    r#"
                                    INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
                                    VALUES (?, ?, 'blake3', 'xchacha20poly1305', strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                                    "#,
                                )
                                .bind(&chunk_hash)
                                .bind(chunk.data.len() as i64)
                                .execute(&mut **conn)
                            )?;

                            let blob = PackBlob {
                                chunk_hash: chunk_hash.clone(),
                                blob: encrypted,
                            };

                            if !pack_enabled {
                                pending_bytes = pending_bytes.saturating_add(blob.blob.len());
                                pending.push(blob);
                                if pending.len() > PACK_ENABLE_MIN_OBJECTS
                                    || pending_bytes > PACK_TARGET_BYTES
                                {
                                    pack_enabled = true;
                                    for b in pending.drain(..) {
                                        schedule_pack_or_direct_upload(
                                            &uploader,
                                            &scan_master_key,
                                            &mut pack_state,
                                            b,
                                        )
                                        .await?;
                                    }
                                    pending_bytes = 0;
                                }
                            } else {
                                schedule_pack_or_direct_upload(
                                    &uploader,
                                    &scan_master_key,
                                    &mut pack_state,
                                    blob,
                                )
                                .await?;
                            }
                        }

                        if pack_enabled && pack_state.should_flush_due_to_age() {
                            flush_packer(&uploader, &scan_master_key, &mut pack_state).await?;
                        }

                        file_chunk_rows.push(FileChunkRow {
                            seq: seq as i64,
                            chunk_hash,
                            offset: chunk.offset as i64,
                            len: chunk.length as i64,
                        });

                    }

                    insert_file_chunks_batch(conn, &file_id, &file_chunk_rows).await?;
                }

                if !pending_base_chunk_copies.is_empty() {
                    let (copied_chunks, deduped_bytes) =
                        flush_base_chunk_copy_batch(conn, &mut pending_base_chunk_copies).await?;
                    if copied_chunks > 0 {
                        result.chunks_total = result.chunks_total.saturating_add(copied_chunks);
                        scan_chunks_total.store(result.chunks_total, Ordering::Relaxed);
                    }
                    if deduped_bytes > 0 {
                        result.bytes_deduped = result.bytes_deduped.saturating_add(deduped_bytes);
                        scan_bytes_deduped.store(result.bytes_deduped, Ordering::Relaxed);
                    }
                }

                debug!(
                    event = "phase.finish",
                    phase = "scan",
                    duration_ms = scan_started.elapsed().as_millis() as u64,
                    files_total = result.files_total,
                    files_indexed = result.files_indexed,
                    chunks_total = result.chunks_total,
                    bytes_read = result.bytes_read,
                    "phase.finish"
                );

                let upload_started = Instant::now();
                debug!(event = "phase.start", phase = "upload", "phase.start");
                if let Some(sink) = options.progress {
                    sink.on_progress(TaskProgress {
                        phase: "upload".to_string(),
                        files_total: None,
                        files_done: Some(scan_files_indexed.load(Ordering::Relaxed)),
                        source_files_total,
                        source_bytes_total,
	                        chunks_total: Some(scan_chunks_total.load(Ordering::Relaxed)),
	                        chunks_done: Some(scan_chunks_total.load(Ordering::Relaxed)),
	                        bytes_read: Some(scan_bytes_read.load(Ordering::Relaxed)),
	                        bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
	                        bytes_downloaded: None,
	                        net_bytes_uploaded: have_uploaded_net_bytes
	                            .load(Ordering::Relaxed)
	                            .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
	                        net_bytes_downloaded: None,
	                        bytes_deduped: Some(scan_bytes_deduped.load(Ordering::Relaxed)),
	                    });
	                }
                if pack_enabled {
                    flush_packer(&uploader, &scan_master_key, &mut pack_state).await?;
                } else {
                    for blob in pending {
                        uploader.enqueue_direct(blob.chunk_hash, blob.blob).await?;
                    }
                }
                drop(upload_tx);

                Ok((snapshot_id, result, upload_started))
            }
            .await;

            if res.is_err() {
                cancel.cancel();
                drop(upload_tx_for_error);
            }
            scan_done.store(true, Ordering::Relaxed);
            res
        }
    };

    drop(uploader);
    drop(upload_tx);

    #[derive(Default)]
    struct UploadStats {
        chunks_uploaded: u64,
        data_objects_uploaded: u64,
        bytes_uploaded: u64,
        first_error: Option<Error>,
        chunk_objects: Vec<ChunkObjectMapping>,
    }

    let collect_future = {
        let scan_files_indexed = Arc::clone(&scan_files_indexed);
        let scan_chunks_total = Arc::clone(&scan_chunks_total);
        let scan_bytes_read = Arc::clone(&scan_bytes_read);
        let scan_bytes_deduped = Arc::clone(&scan_bytes_deduped);
        let uploaded_bytes = Arc::clone(&uploaded_bytes);
        let uploaded_net_bytes = Arc::clone(&uploaded_net_bytes);
        let have_uploaded_net_bytes = Arc::clone(&have_uploaded_net_bytes);
        async move {
            let mut stats = UploadStats::default();
            let mut rx = result_rx;
            while let Some(outcome) = rx.recv().await {
                match outcome {
                    Ok(UploadOutcome::Direct {
                        chunk_hash,
                        object_id,
                        bytes,
                    }) => {
                        stats.chunk_objects.push(ChunkObjectMapping {
                            chunk_hash,
                            object_id: encode_tgfile_object_id(&object_id),
                        });
                        stats.chunks_uploaded += 1;
                        stats.data_objects_uploaded += 1;
                        stats.bytes_uploaded += bytes;
                    }
                    Ok(UploadOutcome::Pack {
                        entries,
                        pack_object_id,
                        bytes,
                    }) => {
                        for entry in entries {
                            stats.chunk_objects.push(ChunkObjectMapping {
                                chunk_hash: entry.chunk_hash,
                                object_id: encode_tgpack_object_id(
                                    &pack_object_id,
                                    entry.offset,
                                    entry.len,
                                ),
                            });
                            stats.chunks_uploaded += 1;
                        }
                        stats.data_objects_uploaded += 1;
                        stats.bytes_uploaded += bytes;
                    }
                    Err(e) => {
                        if stats.first_error.is_none() {
                            stats.first_error = Some(e);
                        }
                    }
                }

                if let Some(sink) = options.progress {
                    sink.on_progress(TaskProgress {
                        phase: "upload".to_string(),
                        files_total: None,
                        files_done: Some(scan_files_indexed.load(Ordering::Relaxed)),
                        source_files_total,
                        source_bytes_total,
                        chunks_total: Some(scan_chunks_total.load(Ordering::Relaxed)),
                        chunks_done: Some(scan_chunks_total.load(Ordering::Relaxed)),
                        bytes_read: Some(scan_bytes_read.load(Ordering::Relaxed)),
                        bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
                        net_bytes_uploaded: have_uploaded_net_bytes
                            .load(Ordering::Relaxed)
                            .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
                        bytes_downloaded: None,
                        net_bytes_downloaded: None,
                        bytes_deduped: Some(scan_bytes_deduped.load(Ordering::Relaxed)),
                    });
                }
            }

            Ok::<UploadStats, Error>(stats)
        }
    };

    let workers_future = async { while workers.next().await.is_some() {} };

    let progress_future = {
        let cancel = upload_cancel.clone();
        let scan_done = Arc::clone(&scan_done);
        let active_uploads = Arc::clone(&active_uploads);
        let pending_jobs = Arc::clone(&pending_jobs);
        let scan_files_indexed = Arc::clone(&scan_files_indexed);
        let scan_chunks_total = Arc::clone(&scan_chunks_total);
        let scan_bytes_read = Arc::clone(&scan_bytes_read);
        let scan_bytes_deduped = Arc::clone(&scan_bytes_deduped);
        let uploaded_bytes = Arc::clone(&uploaded_bytes);
        let uploaded_net_bytes = Arc::clone(&uploaded_net_bytes);
        let have_uploaded_net_bytes = Arc::clone(&have_uploaded_net_bytes);
        async move {
            let Some(sink) = options.progress else {
                return;
            };

            let mut last_uploaded = uploaded_bytes.load(Ordering::Relaxed);
            let mut last_net = have_uploaded_net_bytes
                .load(Ordering::Relaxed)
                .then_some(uploaded_net_bytes.load(Ordering::Relaxed));
            let mut last_emit = Instant::now();
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            loop {
                interval.tick().await;

                let uploaded = uploaded_bytes.load(Ordering::Relaxed);
                let net = have_uploaded_net_bytes
                    .load(Ordering::Relaxed)
                    .then_some(uploaded_net_bytes.load(Ordering::Relaxed));
                let stale = last_emit.elapsed() >= Duration::from_secs(1);
                if uploaded != last_uploaded || net != last_net || stale {
                    last_uploaded = uploaded;
                    last_net = net;
                    last_emit = Instant::now();
                    let phase = if scan_done.load(Ordering::Relaxed) {
                        "upload"
                    } else {
                        "scan"
                    };
                    sink.on_progress(TaskProgress {
                        phase: phase.to_string(),
                        files_total: None,
                        files_done: Some(scan_files_indexed.load(Ordering::Relaxed)),
                        source_files_total,
                        source_bytes_total,
                        chunks_total: Some(scan_chunks_total.load(Ordering::Relaxed)),
                        chunks_done: Some(scan_chunks_total.load(Ordering::Relaxed)),
                        bytes_read: Some(scan_bytes_read.load(Ordering::Relaxed)),
                        bytes_uploaded: Some(uploaded),
                        net_bytes_uploaded: net,
                        bytes_downloaded: None,
                        net_bytes_downloaded: None,
                        bytes_deduped: Some(scan_bytes_deduped.load(Ordering::Relaxed)),
                    });
                }

                if scan_done.load(Ordering::Relaxed)
                    && active_uploads.load(Ordering::Relaxed) == 0
                    && (pending_jobs.load(Ordering::Relaxed) == 0 || cancel.is_cancelled())
                {
                    break;
                }
            }
        }
    };

    let adaptive_future = {
        let cancel = upload_cancel.clone();
        let scan_done = Arc::clone(&scan_done);
        let active_uploads = Arc::clone(&active_uploads);
        let pending_jobs = Arc::clone(&pending_jobs);
        let pending_bytes = Arc::clone(&pending_bytes);
        let uploaded_bytes = Arc::clone(&uploaded_bytes);
        let adaptive = Arc::clone(&adaptive_controller);
        async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(ADAPTIVE_TICK_INTERVAL_SECS));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

            let started = Instant::now();
            let mut last_uploaded = uploaded_bytes.load(Ordering::Relaxed);
            let mut last_tick = Instant::now();
            let mut last_backlog = pending_jobs.load(Ordering::Relaxed) > 0;

            loop {
                let backlog_jobs = pending_jobs.load(Ordering::Relaxed);
                if scan_done.load(Ordering::Relaxed)
                    && active_uploads.load(Ordering::Relaxed) == 0
                    && (backlog_jobs == 0 || cancel.is_cancelled())
                {
                    break;
                }

                let ticked = tokio::select! {
                    _ = interval.tick() => true,
                    _ = sleep(Duration::from_millis(250)) => false,
                };
                if !ticked {
                    continue;
                }

                let now = Instant::now();
                let elapsed = now.saturating_duration_since(last_tick);
                last_tick = now;

                let uploaded_now = uploaded_bytes.load(Ordering::Relaxed);
                let uploaded_delta = uploaded_now.saturating_sub(last_uploaded);
                last_uploaded = uploaded_now;
                let throughput_bps = if elapsed.is_zero() {
                    0
                } else {
                    (uploaded_delta as f64 / elapsed.as_secs_f64()) as u64
                };

                let metrics = adaptive.take_window_metrics();
                let error_rate = if metrics.attempts == 0 {
                    0.0
                } else {
                    metrics.failures as f64 / metrics.attempts as f64
                };
                let backlog_jobs = pending_jobs.load(Ordering::Relaxed);
                let backlog_sustained = backlog_jobs > 0 && last_backlog;
                last_backlog = backlog_jobs > 0;

                let warmup_done = started.elapsed() >= Duration::from_secs(ADAPTIVE_WARMUP_SECS);
                let mut action = if warmup_done { "steady" } else { "warmup" };
                if warmup_done {
                    if error_rate > ADAPTIVE_DOWNGRADE_MIN_ERROR_RATE
                        || metrics.consecutive_failures >= ADAPTIVE_CONSECUTIVE_FAILURES_DOWNGRADE
                    {
                        if adaptive
                            .try_shift_down(ADAPTIVE_DOWNSHIFT_DELAY_STEP_MS)
                            .changed
                        {
                            action = "downshift";
                        }
                    } else if error_rate < ADAPTIVE_UPGRADE_MAX_ERROR_RATE
                        && backlog_jobs > 0
                        && throughput_bps < ADAPTIVE_UPGRADE_THROUGHPUT_BPS
                        && adaptive.try_shift_up().changed
                    {
                        action = "upshift";
                    }
                }

                debug!(
                    event = "upload.adaptive.tick",
                    action,
                    warmup_done,
                    attempts = metrics.attempts,
                    failures = metrics.failures,
                    error_rate,
                    consecutive_failures = metrics.consecutive_failures,
                    backlog_jobs,
                    backlog_bytes = pending_bytes.load(Ordering::Relaxed),
                    backlog_sustained,
                    throughput_bps,
                    target_concurrency = adaptive.target_concurrency(),
                    effective_concurrency = active_uploads.load(Ordering::Relaxed),
                    min_delay_ms = adaptive.min_delay_ms(),
                    "upload.adaptive.tick"
                );

                if scan_done.load(Ordering::Relaxed)
                    && active_uploads.load(Ordering::Relaxed) == 0
                    && (backlog_jobs == 0 || cancel.is_cancelled())
                {
                    break;
                }
            }
        }
    };

    let (scan_res, _, upload_stats, _, _) = tokio::join!(
        scan_future,
        workers_future,
        collect_future,
        progress_future,
        adaptive_future
    );
    let (snapshot_id, mut result, upload_started) = scan_res?;

    let upload_stats = upload_stats?;
    let UploadStats {
        chunks_uploaded,
        data_objects_uploaded,
        bytes_uploaded,
        first_error,
        chunk_objects,
    } = upload_stats;

    if let Err(tail_err) =
        record_chunk_objects_batch(&mut conn, &provider_owned, &chunk_objects).await
    {
        if let Some(upload_err) = first_error {
            error!(
                event = "backup.upload_error_preserved",
                upload_error = %upload_err,
                tail_error = %tail_err,
                "backup.upload_error_preserved"
            );
            return Err(upload_err);
        }
        return Err(tail_err);
    }

    if let Some(err) = first_error {
        return Err(err);
    }

    result.chunks_uploaded = chunks_uploaded;
    result.data_objects_uploaded = data_objects_uploaded;
    result.bytes_uploaded = bytes_uploaded;

    result.data_objects_estimated_without_pack = result.chunks_uploaded;
    debug!(
        event = "phase.finish",
        phase = "upload",
        duration_ms = upload_started.elapsed().as_millis() as u64,
        chunks_uploaded = result.chunks_uploaded,
        data_objects_uploaded = result.data_objects_uploaded,
        bytes_uploaded = result.bytes_uploaded,
        bytes_deduped = result.bytes_deduped,
        "phase.finish"
    );

    if let Some(sink) = options.progress {
        sink.on_progress(TaskProgress {
            phase: "index".to_string(),
            files_total: None,
            files_done: Some(result.files_indexed),
            source_files_total,
            source_bytes_total,
            chunks_total: Some(result.chunks_total),
            chunks_done: Some(result.chunks_total),
            bytes_read: Some(result.bytes_read),
            bytes_uploaded: Some(result.bytes_uploaded),
            net_bytes_uploaded: have_uploaded_net_bytes
                .load(Ordering::Relaxed)
                .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
            bytes_downloaded: None,
            net_bytes_downloaded: None,
            bytes_deduped: Some(result.bytes_deduped),
        });
    }

    let index_started = Instant::now();
    debug!(event = "phase.start", phase = "index", "phase.start");
    let manifest = upload_index(
        &mut conn,
        storage,
        &config,
        &snapshot_id,
        &rate_limiter,
        uploaded_bytes.as_ref(),
        uploaded_net_bytes.as_ref(),
        have_uploaded_net_bytes.as_ref(),
        options.progress,
        source_files_total,
        source_bytes_total,
        scan_files_indexed.load(Ordering::Relaxed),
        scan_chunks_total.load(Ordering::Relaxed),
        scan_bytes_read.load(Ordering::Relaxed),
        scan_bytes_deduped.load(Ordering::Relaxed),
    )
    .await?;
    result.index_parts = manifest.parts.len() as u64;
    result.bytes_uploaded = uploaded_bytes.load(Ordering::Relaxed);

    apply_retention(&mut conn, &config.source_path, config.keep_last_snapshots).await?;

    debug!(
        event = "phase.finish",
        phase = "index",
        duration_ms = index_started.elapsed().as_millis() as u64,
        index_parts = result.index_parts,
        "phase.finish"
    );

    Ok(result)
}

async fn schedule_pack_or_direct_upload(
    uploader: &UploadQueue,
    master_key: &[u8; 32],
    pack_state: &mut PackState,
    blob: PackBlob,
) -> Result<()> {
    let PackBlob { chunk_hash, blob } = blob;

    if blob.len() + SINGLE_BLOB_PACK_OVERHEAD_BUDGET_BYTES > PACK_MAX_BYTES {
        flush_packer(uploader, master_key, pack_state).await?;
        uploader.enqueue_direct(chunk_hash, blob).await?;
        return Ok(());
    }

    if !pack_state.packer.is_empty() && pack_state.packer.blob_len() + blob.len() > PACK_MAX_BYTES {
        flush_packer(uploader, master_key, pack_state).await?;
    }

    pack_state.packer.push_blob(PackBlob { chunk_hash, blob })?;
    pack_state.mark_staged();
    if pack_state.packer.entries_len() >= PACK_MAX_ENTRIES_PER_PACK
        || pack_state.packer.blob_len() >= pack_state.flush_target_bytes
    {
        flush_packer(uploader, master_key, pack_state).await?;
    }

    Ok(())
}

async fn flush_packer(
    uploader: &UploadQueue,
    master_key: &[u8; 32],
    pack_state: &mut PackState,
) -> Result<()> {
    while !pack_state.packer.is_empty() {
        let (pack, carry) = pack_state.packer.finalize_fit(master_key, PACK_MAX_BYTES)?;
        let entries = pack
            .entries
            .into_iter()
            .map(|entry| PackEntryRef {
                chunk_hash: entry.chunk_hash,
                offset: entry.offset,
                len: entry.len,
            })
            .collect::<Vec<_>>();
        uploader.enqueue_pack(entries, pack.bytes).await?;

        pack_state.packs_uploaded = pack_state.packs_uploaded.saturating_add(1);
        pack_state.flush_target_bytes = pack_state.jittered_target_bytes();
        pack_state.packer.reset();
        pack_state.staged_since = None;
        for b in carry {
            pack_state.packer.push_blob(b)?;
        }
        pack_state.mark_staged();
    }
    Ok(())
}

struct PackState {
    packer: PackBuilder,
    packs_uploaded: u64,
    flush_target_bytes: usize,
    seed_prefix: String,
    staged_since: Option<Instant>,
}

impl PackState {
    fn new(provider: &str, snapshot_id: &str) -> Self {
        let seed_prefix = format!("pack_target_bytes|{provider}|{snapshot_id}|");
        let mut state = Self {
            packer: PackBuilder::new(),
            packs_uploaded: 0,
            flush_target_bytes: PACK_TARGET_BYTES,
            seed_prefix,
            staged_since: None,
        };
        state.flush_target_bytes = state.jittered_target_bytes();
        state
    }

    fn jittered_target_bytes(&self) -> usize {
        let seed = format!("{}{}", self.seed_prefix, self.packs_uploaded);
        let h = blake3::hash(seed.as_bytes());
        let mut bytes8 = [0u8; 8];
        bytes8.copy_from_slice(&h.as_bytes()[..8]);
        let v = u64::from_le_bytes(bytes8);

        let base = PACK_TARGET_BYTES as i64;
        let jitter = PACK_TARGET_JITTER_BYTES as i64;
        let span = (PACK_TARGET_JITTER_BYTES as u64) * 2 + 1;
        let offset = (v % span) as i64 - jitter;

        (base + offset) as usize
    }

    fn mark_staged(&mut self) {
        if self.staged_since.is_none() && !self.packer.is_empty() {
            self.staged_since = Some(Instant::now());
        }
    }

    fn should_flush_due_to_age(&self) -> bool {
        let Some(since) = self.staged_since else {
            return false;
        };
        !self.packer.is_empty() && since.elapsed() >= Duration::from_secs(PACK_MAX_STAGING_AGE_SECS)
    }
}

async fn insert_file_chunks_batch(
    conn: &mut DbConn,
    file_id: &str,
    rows: &[FileChunkRow],
) -> Result<()> {
    if rows.is_empty() {
        return Ok(());
    }

    let mut retry_idx = 0usize;
    'retry: loop {
        let mut tx = conn.begin().await.map_err(Error::from)?;

        for row in rows {
            if let Err(e) = sqlx::query(
                r#"
                INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len)
                VALUES (?, ?, ?, ?, ?)
                "#,
            )
            .bind(file_id)
            .bind(row.seq)
            .bind(&row.chunk_hash)
            .bind(row.offset)
            .bind(row.len)
            .execute(&mut *tx)
            .await
            {
                let _ = tx.rollback().await;
                if is_sqlite_busy_or_locked(&e) && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len() {
                    let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                    retry_idx += 1;
                    debug!(
                        event = "sqlite.busy_retry",
                        op = "file_chunks.insert.batch",
                        retry = retry_idx,
                        wait_ms,
                        "sqlite.busy_retry"
                    );
                    sleep(Duration::from_millis(wait_ms)).await;
                    continue 'retry;
                }
                return Err(Error::from(e));
            }
        }

        if let Err(e) = tx.commit().await {
            if is_sqlite_busy_or_locked(&e) && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len() {
                let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                retry_idx += 1;
                debug!(
                    event = "sqlite.busy_retry",
                    op = "file_chunks.insert.batch",
                    retry = retry_idx,
                    wait_ms,
                    "sqlite.busy_retry"
                );
                sleep(Duration::from_millis(wait_ms)).await;
                continue 'retry;
            }
            return Err(Error::from(e));
        }
        return Ok(());
    }
}

async fn record_chunk_objects_batch(
    conn: &mut DbConn,
    provider: &str,
    chunk_objects: &[ChunkObjectMapping],
) -> Result<()> {
    if chunk_objects.is_empty() {
        return Ok(());
    }

    let mut retry_idx = 0usize;
    loop {
        let mut tx = match conn.begin().await {
            Ok(tx) => tx,
            Err(e)
                if is_sqlite_busy_or_locked(&e)
                    && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len() =>
            {
                let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                retry_idx += 1;
                debug!(
                    event = "sqlite.busy_retry",
                    operation = "chunk_objects.begin_tx",
                    retry = retry_idx,
                    wait_ms,
                    error = %e,
                    "sqlite.busy_retry"
                );
                sleep(Duration::from_millis(wait_ms)).await;
                continue;
            }
            Err(e) => return Err(Error::Sqlite(e)),
        };
        let mut retry_err: Option<sqlx::Error> = None;

        for m in chunk_objects {
            if let Err(e) = sqlx::query(
                r#"
                INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at)
                VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                ON CONFLICT(provider, chunk_hash) DO UPDATE SET
                  object_id = excluded.object_id,
                  created_at = excluded.created_at
                "#,
            )
            .bind(&m.chunk_hash)
            .bind(provider)
            .bind(&m.object_id)
            .execute(&mut *tx)
            .await
            {
                retry_err = Some(e);
                break;
            }
        }

        if retry_err.is_none()
            && let Err(e) = tx.commit().await
        {
            retry_err = Some(e);
        }

        if let Some(e) = retry_err {
            if is_sqlite_busy_or_locked(&e) && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len() {
                let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                retry_idx += 1;
                debug!(
                    event = "sqlite.busy_retry",
                    operation = "chunk_objects.upsert_batch",
                    retry = retry_idx,
                    wait_ms,
                    error = %e,
                    "sqlite.busy_retry"
                );
                sleep(Duration::from_millis(wait_ms)).await;
                continue;
            }
            return Err(Error::Sqlite(e));
        }

        return Ok(());
    }
}

async fn apply_retention(
    conn: &mut DbConn,
    source_path: &Path,
    keep_last_snapshots: u32,
) -> Result<()> {
    let source = path_to_utf8(source_path)?;
    let rows = sqlx::query(
        r#"
        SELECT snapshot_id
        FROM snapshots
        WHERE source_path = ?
        ORDER BY created_at DESC
        LIMIT -1 OFFSET ?
        "#,
    )
    .bind(source)
    .bind(keep_last_snapshots as i64)
    .fetch_all(&mut **conn)
    .await?;

    if rows.is_empty() {
        return Ok(());
    }

    let snapshot_ids = rows
        .into_iter()
        .map(|row| row.get::<String, _>("snapshot_id"))
        .collect::<Vec<_>>();

    let mut retry_idx = 0usize;
    loop {
        let mut tx = match conn.begin().await {
            Ok(tx) => tx,
            Err(e)
                if is_sqlite_busy_or_locked(&e)
                    && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len() =>
            {
                let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                retry_idx += 1;
                debug!(
                    event = "sqlite.busy_retry",
                    operation = "snapshots.retention_begin_tx",
                    retry = retry_idx,
                    wait_ms,
                    error = %e,
                    "sqlite.busy_retry"
                );
                sleep(Duration::from_millis(wait_ms)).await;
                continue;
            }
            Err(e) => return Err(Error::Sqlite(e)),
        };
        let mut retry_err: Option<sqlx::Error> = None;
        for snapshot_id in &snapshot_ids {
            if let Err(e) = sqlx::query(
                r#"
                DELETE FROM file_chunks
                WHERE file_id IN (SELECT file_id FROM files WHERE snapshot_id = ?)
                "#,
            )
            .bind(snapshot_id)
            .execute(&mut *tx)
            .await
            {
                retry_err = Some(e);
                break;
            }

            if let Err(e) = sqlx::query("DELETE FROM files WHERE snapshot_id = ?")
                .bind(snapshot_id)
                .execute(&mut *tx)
                .await
            {
                retry_err = Some(e);
                break;
            }

            if let Err(e) = sqlx::query("DELETE FROM remote_index_parts WHERE snapshot_id = ?")
                .bind(snapshot_id)
                .execute(&mut *tx)
                .await
            {
                retry_err = Some(e);
                break;
            }

            if let Err(e) = sqlx::query("DELETE FROM remote_indexes WHERE snapshot_id = ?")
                .bind(snapshot_id)
                .execute(&mut *tx)
                .await
            {
                retry_err = Some(e);
                break;
            }

            if let Err(e) = sqlx::query("DELETE FROM tasks WHERE snapshot_id = ?")
                .bind(snapshot_id)
                .execute(&mut *tx)
                .await
            {
                retry_err = Some(e);
                break;
            }

            if let Err(e) = sqlx::query("DELETE FROM snapshots WHERE snapshot_id = ?")
                .bind(snapshot_id)
                .execute(&mut *tx)
                .await
            {
                retry_err = Some(e);
                break;
            }

            if let Err(e) = sqlx::query(
                "UPDATE snapshots SET base_snapshot_id = NULL WHERE base_snapshot_id = ?",
            )
            .bind(snapshot_id)
            .execute(&mut *tx)
            .await
            {
                retry_err = Some(e);
                break;
            }
        }

        if retry_err.is_none()
            && let Err(e) = tx.commit().await
        {
            retry_err = Some(e);
        }

        if let Some(e) = retry_err {
            if is_sqlite_busy_or_locked(&e) && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len() {
                let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                retry_idx += 1;
                debug!(
                    event = "sqlite.busy_retry",
                    operation = "snapshots.retention",
                    retry = retry_idx,
                    wait_ms,
                    error = %e,
                    "sqlite.busy_retry"
                );
                sleep(Duration::from_millis(wait_ms)).await;
                continue;
            }
            return Err(Error::Sqlite(e));
        }

        return Ok(());
    }
}

async fn latest_snapshot_for_source(
    conn: &mut DbConn,
    source_path: &Path,
) -> Result<Option<String>> {
    let source = path_to_utf8(source_path)?;
    let row = sqlx::query(
        r#"
        SELECT snapshot_id
        FROM snapshots
        WHERE source_path = ?
        ORDER BY created_at DESC
        LIMIT 1
        "#,
    )
    .bind(source)
    .fetch_optional(&mut **conn)
    .await?;

    Ok(row.map(|r| r.get::<String, _>("snapshot_id")))
}

async fn lookup_base_file_snapshot_row(
    conn: &mut DbConn,
    base_snapshot_id: &str,
    rel_path: &str,
) -> Result<Option<BaseFileSnapshotRow>> {
    let row = execute_sqlite_with_busy_retry!(
        "files.lookup_base_snapshot_row",
        sqlx::query(
            r#"
            SELECT file_id, size, mtime_ms, mode
            FROM files
            WHERE snapshot_id = ? AND path = ? AND kind = 'file'
            LIMIT 1
            "#,
        )
        .bind(base_snapshot_id)
        .bind(rel_path)
        .fetch_optional(&mut **conn)
    )?;

    Ok(row.map(|r| BaseFileSnapshotRow {
        file_id: r.get::<String, _>("file_id"),
        size: r.get::<i64, _>("size"),
        mtime_ms: r.get::<i64, _>("mtime_ms"),
        mode: r.get::<i64, _>("mode"),
    }))
}

async fn flush_base_chunk_copy_batch(
    conn: &mut DbConn,
    rows: &mut Vec<BaseFileChunkCopyRow>,
) -> Result<(u64, u64)> {
    if rows.is_empty() {
        return Ok((0, 0));
    }

    let mut retry_idx = 0usize;
    'retry: loop {
        let mut tx = conn.begin().await.map_err(Error::from)?;
        let mut copied_chunks = 0u64;

        for row in rows.iter() {
            let copied = match sqlx::query(
                r#"
                INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len)
                SELECT ?, seq, chunk_hash, offset, len
                FROM file_chunks
                WHERE file_id = ?
                ORDER BY seq
                "#,
            )
            .bind(&row.file_id)
            .bind(&row.base_file_id)
            .execute(&mut *tx)
            .await
            {
                Ok(v) => v.rows_affected(),
                Err(e) => {
                    let _ = tx.rollback().await;
                    if is_sqlite_busy_or_locked(&e) && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len()
                    {
                        let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                        retry_idx += 1;
                        debug!(
                            event = "sqlite.busy_retry",
                            op = "file_chunks.copy_from_base.batch",
                            retry = retry_idx,
                            wait_ms,
                            "sqlite.busy_retry"
                        );
                        sleep(Duration::from_millis(wait_ms)).await;
                        continue 'retry;
                    }
                    return Err(Error::from(e));
                }
            };
            copied_chunks = copied_chunks.saturating_add(copied);
        }

        if let Err(e) = tx.commit().await {
            if is_sqlite_busy_or_locked(&e) && retry_idx < SQLITE_BUSY_RETRY_DELAYS_MS.len() {
                let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[retry_idx];
                retry_idx += 1;
                debug!(
                    event = "sqlite.busy_retry",
                    op = "file_chunks.copy_from_base.batch",
                    retry = retry_idx,
                    wait_ms,
                    "sqlite.busy_retry"
                );
                sleep(Duration::from_millis(wait_ms)).await;
                continue 'retry;
            }
            return Err(Error::from(e));
        }

        let deduped_bytes = rows
            .iter()
            .fold(0u64, |acc, row| acc.saturating_add(row.size));
        rows.clear();
        return Ok((copied_chunks, deduped_bytes));
    }
}

fn telegram_camouflaged_filename() -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    format!("file_{}.dat", &id[..12])
}

fn tgmtproto_peer_from_object_id(object_id: &str) -> Option<String> {
    // chunk_objects.object_id can be:
    // - direct tgmtproto object_id
    // - tgpack slice (tgpack:<tgmtproto...>@off+len)
    // - tgfile wrapper (tgfile:<tgmtproto...>)
    let parsed = match crate::storage::parse_chunk_object_ref(object_id) {
        Ok(v) => v,
        Err(_) => return None,
    };
    let pack_object_id = match parsed {
        crate::storage::ChunkObjectRef::Direct { object_id } => object_id,
        crate::storage::ChunkObjectRef::PackSlice { pack_object_id, .. } => pack_object_id,
    };
    crate::storage::parse_tgmtproto_object_id_v1(&pack_object_id)
        .ok()
        .map(|v| v.peer)
}

async fn load_chunk_hashes_for_storage<S: Storage>(
    conn: &mut DbConn,
    storage: &S,
    provider: &str,
) -> Result<HashSet<String>> {
    let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
        r#"
        SELECT chunk_hash, object_id
        FROM chunk_objects
        WHERE provider = ?
        "#,
    )
    .bind(provider)
    .fetch_all(&mut **conn)
    .await?;

    let mut hashes = HashSet::with_capacity(rows.len());
    let expected_scope = storage.object_id_scope();
    for row in rows {
        let chunk_hash: String = row.get("chunk_hash");
        if let Some(expected_scope) = expected_scope {
            let object_id: String = row.get("object_id");
            // For Telegram MTProto, object IDs embed the peer. If the stored object_id points at
            // a different peer (e.g. user changed endpoint chat_id), treat it as missing so we
            // re-upload and rewrite the mapping.
            let Some(peer) = tgmtproto_peer_from_object_id(&object_id) else {
                continue;
            };
            if peer != expected_scope {
                continue;
            }
        }
        hashes.insert(chunk_hash);
    }
    Ok(hashes)
}

#[allow(clippy::too_many_arguments)]
async fn upload_index<S: Storage>(
    conn: &mut DbConn,
    storage: &S,
    config: &BackupConfig,
    snapshot_id: &str,
    rate_limiter: &UploadRateLimiter,
    uploaded_bytes: &AtomicU64,
    uploaded_net_bytes: &AtomicU64,
    have_uploaded_net_bytes: &AtomicBool,
    progress: Option<&dyn ProgressSink>,
    source_files_total: Option<u64>,
    source_bytes_total: Option<u64>,
    files_indexed: u64,
    chunks_total: u64,
    bytes_read: u64,
    bytes_deduped: u64,
) -> Result<IndexManifest> {
    let provider = storage.provider();
    // Ensure all file/chunk rows are committed, then read the DB file.
    // Note: remote_index_* tables are local cache and may be written after upload.
    let db_bytes = std::fs::read(&config.db_path)?;
    let compressed = zstd::stream::encode_all(db_bytes.as_slice(), 0)?;

    let mut parts = Vec::new();
    for (no, part_plain) in compressed.chunks(INDEX_PART_BYTES).enumerate() {
        let part_no = no as u32;
        let aad = index_part_aad(snapshot_id, part_no);
        let part_enc = encrypt_framed(&config.master_key, aad.as_bytes(), part_plain)?;
        let part_hash = blake3::hash(&part_enc).to_hex().to_string();
        let part_len = part_enc.len();
        let part_len_u64 = part_len as u64;
        let filename = telegram_camouflaged_filename();
        rate_limiter.wait_turn().await;
        let last_reported = AtomicU64::new(0);
        let last_reported_net = AtomicU64::new(0);
        let object_id = storage
            .upload_document_with_progress(
                &filename,
                part_enc,
                Some(Box::new(|p| {
                    let mut progressed = false;

                    let n = p.bytes;
                    let prev = last_reported.swap(n, Ordering::Relaxed);
                    if n > prev {
                        progressed = true;
                        uploaded_bytes.fetch_add(n - prev, Ordering::Relaxed);
                    }

                    if let Some(net) = p.net_bytes {
                        have_uploaded_net_bytes.store(true, Ordering::Relaxed);
                        let prev_net = last_reported_net.swap(net, Ordering::Relaxed);
                        if net > prev_net {
                            progressed = true;
                            uploaded_net_bytes.fetch_add(net - prev_net, Ordering::Relaxed);
                        }
                    }

                    if progressed && let Some(sink) = progress {
                        sink.on_progress(TaskProgress {
                            phase: "index".to_string(),
                            files_total: None,
                            files_done: Some(files_indexed),
                            source_files_total,
                            source_bytes_total,
                            chunks_total: Some(chunks_total),
                            chunks_done: Some(chunks_total),
                            bytes_read: Some(bytes_read),
                            bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
                            net_bytes_uploaded: have_uploaded_net_bytes
                                .load(Ordering::Relaxed)
                                .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
                            bytes_downloaded: None,
                            net_bytes_downloaded: None,
                            bytes_deduped: Some(bytes_deduped),
                        });
                    }
                })),
            )
            .await
            .map_err(|e| {
                error!(
                    event = "io.telegram.upload_failed",
                    provider,
                    snapshot_id,
                    part_no,
                    blob_bytes = part_len_u64,
                    error = %e,
                    "io.telegram.upload_failed"
                );
                e
            })?;

        let reported = last_reported.load(Ordering::Relaxed);
        if reported < part_len_u64 {
            uploaded_bytes.fetch_add(part_len_u64 - reported, Ordering::Relaxed);
        }
        if let Some(sink) = progress {
            sink.on_progress(TaskProgress {
                phase: "index".to_string(),
                files_total: None,
                files_done: Some(files_indexed),
                source_files_total,
                source_bytes_total,
                chunks_total: Some(chunks_total),
                chunks_done: Some(chunks_total),
                bytes_read: Some(bytes_read),
                bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
                net_bytes_uploaded: have_uploaded_net_bytes
                    .load(Ordering::Relaxed)
                    .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
                bytes_downloaded: None,
                net_bytes_downloaded: None,
                bytes_deduped: Some(bytes_deduped),
            });
        }

        execute_sqlite_with_busy_retry!(
            "remote_index_parts.upsert",
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO remote_index_parts (snapshot_id, part_no, provider, object_id, size, hash)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(snapshot_id)
            .bind(part_no as i64)
            .bind(provider)
            .bind(&object_id)
            .bind(part_len as i64)
            .bind(&part_hash)
            .execute(&mut **conn)
        )?;

        parts.push(IndexManifestPart {
            no: part_no,
            size: part_len,
            hash: part_hash,
            object_id,
        });
    }

    let manifest = IndexManifest {
        version: 1,
        snapshot_id: snapshot_id.to_string(),
        hash_alg: "blake3".to_string(),
        enc_alg: "xchacha20poly1305".to_string(),
        compression: "zstd".to_string(),
        parts,
    };
    let manifest_json = serde_json::to_vec(&manifest).map_err(|_| Error::InvalidConfig {
        message: "serialize index manifest failed".to_string(),
    })?;

    let manifest_enc = encrypt_framed(&config.master_key, snapshot_id.as_bytes(), &manifest_json)?;
    let manifest_bytes = manifest_enc.len() as u64;
    let manifest_filename = telegram_camouflaged_filename();
    rate_limiter.wait_turn().await;
    let last_reported = AtomicU64::new(0);
    let last_reported_net = AtomicU64::new(0);
    let manifest_object_id = storage
        .upload_document_with_progress(
            &manifest_filename,
            manifest_enc,
            Some(Box::new(|p| {
                let mut progressed = false;

                let n = p.bytes;
                let prev = last_reported.swap(n, Ordering::Relaxed);
                if n > prev {
                    progressed = true;
                    uploaded_bytes.fetch_add(n - prev, Ordering::Relaxed);
                }

                if let Some(net) = p.net_bytes {
                    have_uploaded_net_bytes.store(true, Ordering::Relaxed);
                    let prev_net = last_reported_net.swap(net, Ordering::Relaxed);
                    if net > prev_net {
                        progressed = true;
                        uploaded_net_bytes.fetch_add(net - prev_net, Ordering::Relaxed);
                    }
                }

                if progressed && let Some(sink) = progress {
                    sink.on_progress(TaskProgress {
                        phase: "index".to_string(),
                        files_total: None,
                        files_done: Some(files_indexed),
                        source_files_total,
                        source_bytes_total,
                        chunks_total: Some(chunks_total),
                        chunks_done: Some(chunks_total),
                        bytes_read: Some(bytes_read),
                        bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
                        net_bytes_uploaded: have_uploaded_net_bytes
                            .load(Ordering::Relaxed)
                            .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
                        bytes_downloaded: None,
                        net_bytes_downloaded: None,
                        bytes_deduped: Some(bytes_deduped),
                    });
                }
            })),
        )
        .await
        .map_err(|e| {
            error!(
                event = "io.telegram.upload_failed",
                provider,
                snapshot_id,
                blob_bytes = manifest_bytes,
                error = %e,
                "io.telegram.upload_failed"
            );
            e
        })?;

    let reported = last_reported.load(Ordering::Relaxed);
    if reported < manifest_bytes {
        uploaded_bytes.fetch_add(manifest_bytes - reported, Ordering::Relaxed);
    }
    if let Some(sink) = progress {
        sink.on_progress(TaskProgress {
            phase: "index".to_string(),
            files_total: None,
            files_done: Some(files_indexed),
            source_files_total,
            source_bytes_total,
            chunks_total: Some(chunks_total),
            chunks_done: Some(chunks_total),
            bytes_read: Some(bytes_read),
            bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
            net_bytes_uploaded: have_uploaded_net_bytes
                .load(Ordering::Relaxed)
                .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
            bytes_downloaded: None,
            net_bytes_downloaded: None,
            bytes_deduped: Some(bytes_deduped),
        });
    }

    execute_sqlite_with_busy_retry!(
        "remote_indexes.upsert",
        sqlx::query(
            r#"
            INSERT OR REPLACE INTO remote_indexes (snapshot_id, provider, manifest_object_id, created_at)
            VALUES (?, ?, ?, strftime('%Y-%m-%dT%H:%M:%fZ','now'))
            "#,
        )
        .bind(snapshot_id)
        .bind(provider)
        .bind(&manifest_object_id)
        .execute(&mut **conn)
    )?;

    Ok(manifest)
}

fn path_to_utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::NonUtf8Path {
            path: path.to_path_buf(),
        })
}
