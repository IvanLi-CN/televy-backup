use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use fastcdc::ronomon::FastCDC;
use fastcdc::v2020::{ChunkData, Error as CdcError, MAXIMUM_MAX as V2020_MAXIMUM_MAX, StreamCDC};
use futures::stream::{FuturesUnordered, StreamExt};
use ignore::{Error as IgnoreError, Walk, WalkBuilder};
use serde::{Deserialize, Serialize};
use sqlx::pool::PoolConnection;
use sqlx::{Connection, QueryBuilder, Row, Sqlite, Transaction};
use tracing::{debug, error, info, warn};

use crate::config::TelegramRateLimit;
use crate::crypto::FRAMING_OVERHEAD_BYTES;
use crate::crypto::encrypt_framed;
use crate::dedupe_catalog::{
    DEDUPE_CATALOG_VERSION, DedupeCatalogBase, DedupeCatalogDelta, DedupeCatalogV1,
    dedupe_base_id_for_storage, dedupe_delta_id_from_scope, load_remote_dedupe_catalog,
    save_remote_dedupe_catalog,
};
use crate::index_db::{open_existing_index_db, open_index_db, open_snapshot_filemap_db};
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
// Two bound columns per mapping keep 480 rows below SQLite's usual 999 bind limit.
const BASE_FILE_CHUNK_COPY_BATCH_SIZE: usize = 480;
const SCAN_FILE_METADATA_BATCH_SIZE: usize = 512;
// The bundled SQLite build supports thousands of bind variables; keep every
// statement bounded by the scan batch size so memory and cancellation behavior
// remain predictable.
const SCAN_FILE_INSERT_ROWS_PER_STATEMENT: usize = 512;
const FILE_CHUNK_INSERT_ROWS_PER_STATEMENT: usize = 512;
const FILEMAP_CHUNK_INSERT_ROWS_PER_STATEMENT: usize = 512;
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
const DEDUPE_MAX_DELTAS_BEFORE_COMPACT: usize = 128;
const SQLITE_BUSY_RETRY_DELAYS_MS: [u64; 5] = [100, 250, 500, 1000, 2000];
const UPLOAD_OBJECT_MAX_ATTEMPTS: usize = 3;
const UPLOAD_OBJECT_RETRY_BASE_MS: u64 = 1_000;
const UPLOAD_OBJECT_RETRY_MAX_MS: u64 = 15_000;
static INDEX_UPLOAD_SEQUENCE: AtomicU64 = AtomicU64::new(1);
const CHUNK_OBJECT_CHECKPOINT_BATCH_SIZE: usize = 256;
const INDEX_COMPACT_MIN_PAGE_COUNT: i64 = 131_072; // ~= 512 MiB @ 4 KiB pages
const INDEX_COMPACT_MIN_FREE_PAGES: i64 = 16_384; // ~= 64 MiB @ 4 KiB pages
const INDEX_COMPACT_MIN_FREE_RATIO: f64 = 0.20;
const RETENTION_SNAPSHOT_BATCH_SIZE: usize = 8;
const RETENTION_FILE_BATCH_SIZE: usize = 256;
const TELEVYIGNORE_FILE_NAME: &str = ".televyignore";
const SCAN_TRACE_BUCKET_MS: u64 = 1_000;
const SCAN_TRACE_BUCKET_US: u64 = SCAN_TRACE_BUCKET_MS * 1_000;
const SCAN_TRACE_MAX_BUCKETS: usize = 4_096;

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
    pub endpoint_db_path: PathBuf,
    /// Directory for per-snapshot filemap DBs (one sqlite per snapshot).
    ///
    /// Expected layout (per endpoint): `<filemap_dir>/<snapshot_id>.sqlite`.
    pub filemap_dir: PathBuf,
    /// Local materialized dedupe DB (`chunks` + `chunk_objects`) for this endpoint.
    pub dedupe_db_path: PathBuf,
    /// Local pending dedupe spool DB (used to publish remote dedupe deltas).
    pub dedupe_pending_db_path: PathBuf,
    pub source_path: PathBuf,
    pub label: String,
    pub chunking: ChunkingConfig,
    pub rate_limit: TelegramRateLimit,
    pub master_key: [u8; 32],
    pub snapshot_id: Option<String>,
    pub keep_last_snapshots: u32,
    pub remote_dedupe: RemoteDedupeMode,
}

#[derive(Debug, Clone)]
pub enum RemoteDedupeMode {
    Disabled,
    /// Remote dedupe is not initialized yet; publish a base DB + empty catalog.
    Enable {
        endpoint_dedupe_id: String,
    },
    /// Remote dedupe is initialized; publish per-run delta DBs and update the catalog.
    Incremental {
        endpoint_dedupe_id: String,
        catalog_object_id: String,
    },
}

impl RemoteDedupeMode {
    fn enabled(&self) -> bool {
        !matches!(self, Self::Disabled)
    }
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
    pub ignore_rule_files: u64,
    pub ignore_invalid_rules: u64,
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

fn build_source_walk(source_path: &Path) -> Walk {
    let mut builder = WalkBuilder::new(source_path);
    builder
        .follow_links(false)
        .hidden(false)
        .parents(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .add_custom_ignore_filename(TELEVYIGNORE_FILE_NAME);
    builder.build()
}

fn ignore_error_is_rule_parse_only(err: &IgnoreError) -> bool {
    match err {
        IgnoreError::Partial(errs) => {
            !errs.is_empty() && errs.iter().all(ignore_error_is_rule_parse_only)
        }
        IgnoreError::WithLineNumber { err, .. } => ignore_error_is_rule_parse_only(err),
        IgnoreError::WithPath { err, .. } => ignore_error_is_rule_parse_only(err),
        IgnoreError::WithDepth { err, .. } => ignore_error_is_rule_parse_only(err),
        IgnoreError::Glob { .. } => true,
        _ => false,
    }
}

fn ignore_error_path(err: &IgnoreError) -> Option<&Path> {
    match err {
        IgnoreError::Partial(errs) => errs.iter().find_map(ignore_error_path),
        IgnoreError::WithLineNumber { err, .. } => ignore_error_path(err),
        IgnoreError::WithPath { path, .. } => Some(path.as_path()),
        IgnoreError::WithDepth { err, .. } => ignore_error_path(err),
        _ => None,
    }
}

fn warn_invalid_televyignore_rule_once(
    warned: &mut HashSet<String>,
    err: &IgnoreError,
    source_path: &Path,
    phase: &'static str,
) {
    if !ignore_error_is_rule_parse_only(err) {
        return;
    }
    let key = err.to_string();
    if !warned.insert(key.clone()) {
        return;
    }
    let ignore_file = ignore_error_path(err).unwrap_or_else(|| Path::new(""));
    warn!(
        event = "source.ignore.invalid_rule",
        phase,
        source_path = %source_path.display(),
        ignore_file = %ignore_file.display(),
        error = %key,
        "source.ignore.invalid_rule"
    );
}

fn ignore_error_is_not_found(err: &IgnoreError) -> bool {
    err.io_error()
        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
}

fn ignore_error_is_non_root_not_found(err: &IgnoreError, source_path: &Path) -> bool {
    if !ignore_error_is_not_found(err) {
        return false;
    }
    if err.depth() == Some(0) {
        return false;
    }
    ignore_error_path(err).map_or_else(|| source_path.exists(), |path| path != source_path)
}

fn count_ignore_file_for_dir(seen: &mut HashSet<PathBuf>, dir_path: &Path) -> u64 {
    let ignore_path = dir_path.join(TELEVYIGNORE_FILE_NAME);
    if seen.contains(&ignore_path) {
        return 0;
    }
    let is_file = std::fs::metadata(&ignore_path)
        .map(|meta| meta.is_file())
        .unwrap_or(false);
    if !is_file {
        return 0;
    }
    seen.insert(ignore_path);
    1
}

fn map_ignore_error(err: IgnoreError, source_path: &Path) -> Error {
    Error::Walk {
        message: format!("source walk failed for {}: {err}", source_path.display()),
    }
}

pub fn compute_source_quick_stats(
    source_path: &Path,
    cancel: Option<&CancellationToken>,
) -> Result<SourceQuickStats> {
    let mut files_total = 0u64;
    let mut bytes_total = 0u64;
    let mut warned_ignore_errors = HashSet::<String>::new();

    for entry in build_source_walk(source_path) {
        if let Some(cancel) = cancel
            && cancel.is_cancelled()
        {
            return Err(Error::Cancelled);
        }

        let entry = match entry {
            Ok(v) => v,
            Err(e) => {
                if ignore_error_is_rule_parse_only(&e) {
                    warn_invalid_televyignore_rule_once(
                        &mut warned_ignore_errors,
                        &e,
                        source_path,
                        "prepare",
                    );
                    continue;
                }
                if ignore_error_is_non_root_not_found(&e, source_path) {
                    continue;
                }
                return Err(map_ignore_error(e, source_path));
            }
        };

        if let Some(err) = entry.error() {
            if ignore_error_is_rule_parse_only(err) {
                warn_invalid_televyignore_rule_once(
                    &mut warned_ignore_errors,
                    err,
                    source_path,
                    "prepare",
                );
            } else if ignore_error_is_not_found(err) && entry.path() != source_path {
                continue;
            } else {
                return Err(map_ignore_error(err.clone(), source_path));
            }
        }

        let path = entry.path();
        if path == source_path {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(v) => v,
            Err(e) => {
                if ignore_error_is_not_found(&e) {
                    continue;
                }
                return Err(map_ignore_error(e, source_path));
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

    async fn wait_turn(&self) -> Duration {
        let started = Instant::now();
        let min_delay_ms = self.min_delay_ms();
        if min_delay_ms == 0 {
            return started.elapsed();
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
        started.elapsed()
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
    worker_slots: AtomicUsize,
    window_attempts: AtomicU64,
    window_failures: AtomicU64,
    consecutive_failures: AtomicUsize,
    limiter: Arc<UploadRateLimiter>,
    notify: Notify,
}

struct AdaptiveUploadSlot {
    controller: Arc<AdaptiveUploadController>,
    worker_index: usize,
}

impl Drop for AdaptiveUploadSlot {
    fn drop(&mut self) {
        self.controller
            .worker_slots
            .fetch_and(!(1usize << self.worker_index), Ordering::Relaxed);
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
            worker_slots: AtomicUsize::new(0),
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
                    if let Some(worker_index) = self.try_acquire_worker_index() {
                        return Ok(AdaptiveUploadSlot {
                            controller: Arc::clone(self),
                            worker_index,
                        });
                    }
                    saturating_sub_usize(&self.slots_in_use, 1);
                }
                continue;
            }

            tokio::select! {
                _ = self.notify.notified() => {},
                _ = cancel.cancelled() => return Err(Error::Cancelled),
            }
        }
    }

    fn try_acquire_worker_index(&self) -> Option<usize> {
        loop {
            let occupied = self.worker_slots.load(Ordering::Relaxed);
            for worker_index in 0..self.max_concurrency {
                let mask = 1usize << worker_index;
                if occupied & mask != 0 {
                    continue;
                }
                if self
                    .worker_slots
                    .compare_exchange(
                        occupied,
                        occupied | mask,
                        Ordering::Relaxed,
                        Ordering::Relaxed,
                    )
                    .is_ok()
                {
                    return Some(worker_index);
                }
                break;
            }
            if occupied.count_ones() as usize >= self.max_concurrency {
                return None;
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
        Error::Telegram { message } => {
            let msg = message.to_ascii_uppercase();
            msg.contains("FLOOD_WAIT")
                || msg.contains("FLOOD WAIT")
                || msg.contains("FLOOD_PREMIUM_WAIT")
                || msg.contains("FLOOD PREMIUM WAIT")
        }
        _ => false,
    }
}

fn is_retryable_upload_error(error: &Error) -> bool {
    if error_has_flood_wait(error) {
        return true;
    }
    match error {
        Error::Telegram { message } => crate::error::is_transient_telegram_message(message),
        _ => false,
    }
}

fn upload_object_retry_backoff(attempt: usize) -> Duration {
    let shift = attempt.saturating_sub(1).min(16) as u32;
    let mul = 1u64.checked_shl(shift).unwrap_or(u64::MAX);
    let ms = UPLOAD_OBJECT_RETRY_BASE_MS.saturating_mul(mul);
    Duration::from_millis(ms.min(UPLOAD_OBJECT_RETRY_MAX_MS))
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

#[derive(Debug)]
struct ScanSqliteRetryWait {
    started: Instant,
    finished: Instant,
}

async fn wait_for_scan_sqlite_busy_retry(
    operation: &str,
    retry_idx: &mut usize,
    error: &sqlx::Error,
    retry_waits: &mut Vec<ScanSqliteRetryWait>,
) -> bool {
    if !is_sqlite_busy_or_locked(error) || *retry_idx >= SQLITE_BUSY_RETRY_DELAYS_MS.len() {
        return false;
    }

    let wait_ms = SQLITE_BUSY_RETRY_DELAYS_MS[*retry_idx];
    *retry_idx += 1;
    debug!(
        event = "sqlite.busy_retry",
        operation,
        retry = *retry_idx,
        wait_ms,
        error = %error,
        "sqlite.busy_retry"
    );
    let started = Instant::now();
    sleep(Duration::from_millis(wait_ms)).await;
    retry_waits.push(ScanSqliteRetryWait {
        started,
        finished: Instant::now(),
    });
    true
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

macro_rules! execute_scan_sqlite_with_busy_retry {
    ($op_name:expr, $query:expr) => {{
        let mut retry_idx = 0usize;
        let mut retry_waits = Vec::new();
        loop {
            match $query.await {
                Ok(value) => break Ok((value, retry_waits)),
                Err(error) => {
                    if wait_for_scan_sqlite_busy_retry(
                        $op_name,
                        &mut retry_idx,
                        &error,
                        &mut retry_waits,
                    )
                    .await
                    {
                        continue;
                    }
                    break Err(Error::Sqlite(error));
                }
            }
        }
    }};
}

#[derive(Debug, Clone)]
struct PackEntryRef {
    chunk_hash: String,
    offset: u64,
    len: u64,
    source_bytes: u64,
}

#[derive(Debug)]
enum UploadJob {
    Direct {
        sequence: u64,
        queued_at: Instant,
        chunk_hash: String,
        blob: Vec<u8>,
        source_bytes: u64,
        _bytes_permit: OwnedSemaphorePermit,
    },
    Pack {
        sequence: u64,
        queued_at: Instant,
        entries: Vec<PackEntryRef>,
        pack_bytes: Vec<u8>,
        source_bytes: u64,
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
        source_bytes: u64,
    },
    Pack {
        entries: Vec<PackEntryRef>,
        pack_object_id: String,
        bytes: u64,
        source_bytes: u64,
    },
}

#[derive(Debug, Clone)]
struct ChunkObjectMapping {
    chunk_hash: String,
    object_id: String,
    source_bytes: u64,
}

#[derive(Debug, Clone)]
struct FileChunkRow {
    seq: i64,
    chunk_hash: String,
    offset: i64,
    len: i64,
}

#[derive(Debug, Clone)]
struct FilemapChunkRow {
    chunk_hash: String,
    size: i64,
}

#[derive(Debug, Clone)]
struct SourceBlob {
    chunk_hash: String,
    blob: Vec<u8>,
    source_bytes: u64,
}

#[derive(Debug, Clone, Copy)]
enum ScanWorkKind {
    Walk,
    Metadata,
    ReadChunk,
    Hash,
    Encrypt,
    Sqlite,
    SqliteRetryWait,
}

#[derive(Debug, Clone, Default, Serialize)]
struct ScanTraceBucket {
    offset_ms: u64,
    #[serde(skip_serializing_if = "is_zero")]
    walk_us: u64,
    #[serde(skip_serializing_if = "is_zero")]
    metadata_us: u64,
    #[serde(skip_serializing_if = "is_zero")]
    read_chunk_us: u64,
    #[serde(skip_serializing_if = "is_zero")]
    hash_us: u64,
    #[serde(skip_serializing_if = "is_zero")]
    encrypt_us: u64,
    #[serde(skip_serializing_if = "is_zero")]
    sqlite_us: u64,
    #[serde(skip_serializing_if = "is_zero")]
    sqlite_retry_wait_us: u64,
}

impl ScanTraceBucket {
    fn add(&mut self, kind: ScanWorkKind, elapsed_us: u64) {
        let slot = match kind {
            ScanWorkKind::Walk => &mut self.walk_us,
            ScanWorkKind::Metadata => &mut self.metadata_us,
            ScanWorkKind::ReadChunk => &mut self.read_chunk_us,
            ScanWorkKind::Hash => &mut self.hash_us,
            ScanWorkKind::Encrypt => &mut self.encrypt_us,
            ScanWorkKind::Sqlite => &mut self.sqlite_us,
            ScanWorkKind::SqliteRetryWait => &mut self.sqlite_retry_wait_us,
        };
        *slot = slot.saturating_add(elapsed_us);
    }

    fn merge_from(&mut self, other: &Self) {
        self.walk_us = self.walk_us.saturating_add(other.walk_us);
        self.metadata_us = self.metadata_us.saturating_add(other.metadata_us);
        self.read_chunk_us = self.read_chunk_us.saturating_add(other.read_chunk_us);
        self.hash_us = self.hash_us.saturating_add(other.hash_us);
        self.encrypt_us = self.encrypt_us.saturating_add(other.encrypt_us);
        self.sqlite_us = self.sqlite_us.saturating_add(other.sqlite_us);
        self.sqlite_retry_wait_us = self
            .sqlite_retry_wait_us
            .saturating_add(other.sqlite_retry_wait_us);
    }
}

#[derive(Debug, Serialize)]
struct ScanTrace {
    version: u8,
    resolution_ms: u64,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    sqlite_ops_ms: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    sqlite_ops_count: BTreeMap<String, u64>,
    buckets: Vec<ScanTraceBucket>,
}

#[derive(Debug)]
struct ScanActivityTrace {
    started: Instant,
    bucket_width_us: u64,
    buckets: Vec<ScanTraceBucket>,
}

impl ScanActivityTrace {
    fn new(started: Instant) -> Self {
        Self {
            started,
            bucket_width_us: SCAN_TRACE_BUCKET_US,
            buckets: Vec::new(),
        }
    }

    fn record(&mut self, kind: ScanWorkKind, operation_started: Instant) -> u64 {
        self.record_between(kind, operation_started, Instant::now())
    }

    fn record_between(
        &mut self,
        kind: ScanWorkKind,
        operation_started: Instant,
        operation_finished: Instant,
    ) -> u64 {
        let start_us = operation_started
            .saturating_duration_since(self.started)
            .as_micros() as u64;
        let end_us = operation_finished
            .saturating_duration_since(self.started)
            .as_micros() as u64;
        self.record_interval_us(kind, start_us.min(end_us), end_us);

        end_us.saturating_sub(start_us)
    }

    fn record_interval_us(&mut self, kind: ScanWorkKind, start_us: u64, end_us: u64) {
        if start_us >= end_us {
            return;
        }

        self.coarsen_for(end_us);
        let mut cursor_us = start_us;

        while cursor_us < end_us {
            let bucket_index = (cursor_us / self.bucket_width_us) as usize;
            let bucket_end_us = (bucket_index as u64 + 1).saturating_mul(self.bucket_width_us);
            let segment_end_us = end_us.min(bucket_end_us);
            if self.buckets.len() <= bucket_index {
                self.buckets
                    .resize_with(bucket_index + 1, || ScanTraceBucket {
                        offset_ms: 0,
                        ..ScanTraceBucket::default()
                    });
            }
            let bucket = &mut self.buckets[bucket_index];
            bucket.offset_ms = bucket_index as u64 * SCAN_TRACE_BUCKET_MS;
            bucket.add(kind, segment_end_us.saturating_sub(cursor_us));
            cursor_us = segment_end_us;
        }
    }

    fn coarsen_for(&mut self, end_us: u64) {
        while end_us.saturating_sub(1) / self.bucket_width_us >= SCAN_TRACE_MAX_BUCKETS as u64 {
            self.bucket_width_us = self.bucket_width_us.saturating_mul(2);
            let mut buckets = Vec::with_capacity(self.buckets.len().div_ceil(2));
            for (index, bucket) in self.buckets.drain(..).enumerate() {
                let compacted_index = index / 2;
                if buckets.len() <= compacted_index {
                    buckets.push(ScanTraceBucket::default());
                }
                buckets[compacted_index].merge_from(&bucket);
            }
            self.buckets = buckets;
        }
    }

    fn to_json(
        &self,
        sqlite_ops_ms: BTreeMap<String, u64>,
        sqlite_ops_count: BTreeMap<String, u64>,
    ) -> (String, u64) {
        let resolution_ms = self.bucket_width_us / 1_000;
        let mut buckets = self.buckets.clone();
        for (index, bucket) in buckets.iter_mut().enumerate() {
            bucket.offset_ms = index as u64 * resolution_ms;
        }

        let json = serde_json::to_string(&ScanTrace {
            version: 2,
            resolution_ms,
            sqlite_ops_ms,
            sqlite_ops_count,
            buckets,
        })
        .unwrap_or_else(|_| {
            format!("{{\"version\":2,\"resolution_ms\":{resolution_ms},\"buckets\":[]}}")
        });
        (json, resolution_ms)
    }
}

fn is_zero(value: &u64) -> bool {
    *value == 0
}

#[derive(Debug)]
struct ScanPerformance {
    walk_us: u64,
    metadata_us: u64,
    read_chunk_us: u64,
    hash_us: u64,
    encrypt_us: u64,
    sqlite_timed_us: u64,
    sqlite_retry_wait_us: u64,
    sqlite_ops_us: BTreeMap<&'static str, u64>,
    sqlite_ops_count: BTreeMap<&'static str, u64>,
    upload_queue_blocked_ms: u64,
    trace: ScanActivityTrace,
}

impl ScanPerformance {
    fn new(scan_started: Instant) -> Self {
        Self {
            walk_us: 0,
            metadata_us: 0,
            read_chunk_us: 0,
            hash_us: 0,
            encrypt_us: 0,
            sqlite_timed_us: 0,
            sqlite_retry_wait_us: 0,
            sqlite_ops_us: BTreeMap::new(),
            sqlite_ops_count: BTreeMap::new(),
            upload_queue_blocked_ms: 0,
            trace: ScanActivityTrace::new(scan_started),
        }
    }

    fn record(&mut self, kind: ScanWorkKind, operation_started: Instant) {
        let elapsed_us = self.trace.record(kind, operation_started);
        let slot = match kind {
            ScanWorkKind::Walk => &mut self.walk_us,
            ScanWorkKind::Metadata => &mut self.metadata_us,
            ScanWorkKind::ReadChunk => &mut self.read_chunk_us,
            ScanWorkKind::Hash => &mut self.hash_us,
            ScanWorkKind::Encrypt => &mut self.encrypt_us,
            ScanWorkKind::Sqlite => &mut self.sqlite_timed_us,
            ScanWorkKind::SqliteRetryWait => &mut self.sqlite_retry_wait_us,
        };
        *slot = slot.saturating_add(elapsed_us);
    }

    fn record_sqlite(
        &mut self,
        operation: &'static str,
        operation_started: Instant,
        retry_waits: &[ScanSqliteRetryWait],
    ) {
        let operation_finished = Instant::now();
        let mut active_started = operation_started;
        for retry_wait in retry_waits {
            self.record_sqlite_active(operation, active_started, retry_wait.started);
            let wait_us = self.trace.record_between(
                ScanWorkKind::SqliteRetryWait,
                retry_wait.started,
                retry_wait.finished,
            );
            self.sqlite_retry_wait_us = self.sqlite_retry_wait_us.saturating_add(wait_us);
            active_started = retry_wait.finished;
        }
        self.record_sqlite_active(operation, active_started, operation_finished);
        let op_count = self.sqlite_ops_count.entry(operation).or_default();
        *op_count = op_count.saturating_add(1);
    }

    fn record_sqlite_active(
        &mut self,
        operation: &'static str,
        operation_started: Instant,
        operation_finished: Instant,
    ) {
        let elapsed_us =
            self.trace
                .record_between(ScanWorkKind::Sqlite, operation_started, operation_finished);
        self.sqlite_timed_us = self.sqlite_timed_us.saturating_add(elapsed_us);
        let op_duration = self.sqlite_ops_us.entry(operation).or_default();
        *op_duration = op_duration.saturating_add(elapsed_us);
    }

    fn trace_json(&self) -> (String, u64) {
        let sqlite_ops_ms = self
            .sqlite_ops_us
            .iter()
            .map(|(operation, elapsed_us)| (operation.to_string(), elapsed_us / 1_000))
            .collect();
        let sqlite_ops_count = self
            .sqlite_ops_count
            .iter()
            .map(|(operation, count)| (operation.to_string(), *count))
            .collect();
        self.trace.to_json(sqlite_ops_ms, sqlite_ops_count)
    }

    fn attributed_us(&self) -> u64 {
        self.walk_us
            .saturating_add(self.metadata_us)
            .saturating_add(self.read_chunk_us)
            .saturating_add(self.hash_us)
            .saturating_add(self.encrypt_us)
            .saturating_add(self.sqlite_timed_us)
            .saturating_add(self.sqlite_retry_wait_us)
            .saturating_add(self.upload_queue_blocked_ms.saturating_mul(1_000))
    }
}

#[derive(Debug, Clone)]
struct BaseFileSnapshotRow {
    file_id: String,
    size: i64,
    mtime_ms: i64,
    mode: i64,
}

#[derive(Debug)]
struct PendingScanEntry {
    path: PathBuf,
    rel_path: String,
    kind: &'static str,
    size: i64,
    mtime_ms: i64,
    mode: i64,
    file_id: String,
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
    planned_upload_bytes: Arc<AtomicU64>,
    phase_started: Arc<AtomicBool>,
    next_sequence: Arc<AtomicU64>,
    next_queue_wait_id: Arc<AtomicU64>,
    scan_queue_blocked_ms: Arc<AtomicU64>,
    cancel: CancellationToken,
}

impl UploadQueue {
    async fn enqueue_direct(
        &self,
        chunk_hash: String,
        blob: Vec<u8>,
        source_bytes: u64,
    ) -> Result<()> {
        let queue_started = Instant::now();
        let queue_wait_id = self.next_queue_wait_id.fetch_add(1, Ordering::Relaxed);
        let bytes = blob.len();
        info!(
            event = "performance.scan.queue_wait.start",
            phase = "scan",
            queue_wait_id,
            kind = "direct",
            "performance.scan.queue_wait.start"
        );
        let permit =
            match acquire_bytes(&self.bytes_sem, self.bytes_budget, bytes, &self.cancel).await {
                Ok(permit) => permit,
                Err(err) => {
                    info!(
                        event = "performance.scan.queue_wait.finish",
                        phase = "scan",
                        queue_wait_id,
                        kind = "direct",
                        queue_wait_ms = queue_started.elapsed().as_millis() as u64,
                        result = "failed",
                        "performance.scan.queue_wait.finish"
                    );
                    return Err(err);
                }
            };
        self.pending_jobs.fetch_add(1, Ordering::Relaxed);
        self.pending_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        self.planned_upload_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        let job = UploadJob::Direct {
            sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
            queued_at: queue_started,
            chunk_hash,
            blob,
            source_bytes,
            _bytes_permit: permit,
        };
        if self.sender.send(job).await.is_err() {
            saturating_sub_usize(self.pending_jobs.as_ref(), 1);
            saturating_sub_u64(self.pending_bytes.as_ref(), bytes as u64);
            saturating_sub_u64(self.planned_upload_bytes.as_ref(), bytes as u64);
            info!(
                event = "performance.scan.queue_wait.finish",
                phase = "scan",
                queue_wait_id,
                kind = "direct",
                queue_wait_ms = queue_started.elapsed().as_millis() as u64,
                result = "failed",
                "performance.scan.queue_wait.finish"
            );
            return Err(Error::Telegram {
                message: "upload queue closed".to_string(),
            });
        }
        let queue_wait_ms = queue_started.elapsed().as_millis() as u64;
        self.scan_queue_blocked_ms
            .fetch_add(queue_wait_ms, Ordering::Relaxed);
        info!(
            event = "performance.scan.queue_wait.finish",
            phase = "scan",
            queue_wait_id,
            kind = "direct",
            queue_wait_ms,
            result = "enqueued",
            "performance.scan.queue_wait.finish"
        );
        if self
            .phase_started
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            debug!(event = "phase.start", phase = "scan_upload", "phase.start");
        }
        Ok(())
    }

    async fn enqueue_pack(&self, entries: Vec<PackEntryRef>, pack_bytes: Vec<u8>) -> Result<()> {
        let queue_started = Instant::now();
        let queue_wait_id = self.next_queue_wait_id.fetch_add(1, Ordering::Relaxed);
        let bytes = pack_bytes.len();
        let source_bytes = entries
            .iter()
            .fold(0u64, |acc, entry| acc.saturating_add(entry.source_bytes));
        info!(
            event = "performance.scan.queue_wait.start",
            phase = "scan",
            queue_wait_id,
            kind = "pack",
            "performance.scan.queue_wait.start"
        );
        let permit =
            match acquire_bytes(&self.bytes_sem, self.bytes_budget, bytes, &self.cancel).await {
                Ok(permit) => permit,
                Err(err) => {
                    info!(
                        event = "performance.scan.queue_wait.finish",
                        phase = "scan",
                        queue_wait_id,
                        kind = "pack",
                        queue_wait_ms = queue_started.elapsed().as_millis() as u64,
                        result = "failed",
                        "performance.scan.queue_wait.finish"
                    );
                    return Err(err);
                }
            };
        self.pending_jobs.fetch_add(1, Ordering::Relaxed);
        self.pending_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        self.planned_upload_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
        let job = UploadJob::Pack {
            sequence: self.next_sequence.fetch_add(1, Ordering::Relaxed),
            queued_at: queue_started,
            entries,
            pack_bytes,
            source_bytes,
            _bytes_permit: permit,
        };
        if self.sender.send(job).await.is_err() {
            saturating_sub_usize(self.pending_jobs.as_ref(), 1);
            saturating_sub_u64(self.pending_bytes.as_ref(), bytes as u64);
            saturating_sub_u64(self.planned_upload_bytes.as_ref(), bytes as u64);
            info!(
                event = "performance.scan.queue_wait.finish",
                phase = "scan",
                queue_wait_id,
                kind = "pack",
                queue_wait_ms = queue_started.elapsed().as_millis() as u64,
                result = "failed",
                "performance.scan.queue_wait.finish"
            );
            return Err(Error::Telegram {
                message: "upload queue closed".to_string(),
            });
        }
        let queue_wait_ms = queue_started.elapsed().as_millis() as u64;
        self.scan_queue_blocked_ms
            .fetch_add(queue_wait_ms, Ordering::Relaxed);
        info!(
            event = "performance.scan.queue_wait.finish",
            phase = "scan",
            queue_wait_id,
            kind = "pack",
            queue_wait_ms,
            result = "enqueued",
            "performance.scan.queue_wait.finish"
        );
        if self
            .phase_started
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
        {
            debug!(event = "phase.start", phase = "scan_upload", "phase.start");
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

#[allow(clippy::too_many_arguments)]
async fn process_upload_job<S: Storage>(
    storage: &S,
    provider: &str,
    worker_index: usize,
    limiter: &UploadRateLimiter,
    uploaded_bytes: &AtomicU64,
    uploaded_net_bytes: &AtomicU64,
    have_uploaded_net_bytes: &AtomicBool,
    job: UploadJob,
) -> Result<UploadOutcome> {
    match job {
        UploadJob::Direct {
            sequence,
            queued_at,
            chunk_hash,
            blob,
            source_bytes,
            _bytes_permit,
        } => {
            let bytes_len = blob.len() as u64;
            let queue_wait_ms = queued_at.elapsed().as_millis() as u64;
            for attempt in 1..=UPLOAD_OBJECT_MAX_ATTEMPTS {
                let rate_limit_wait_ms = limiter.wait_turn().await.as_millis() as u64;
                info!(
                    event = "performance.upload.rate_limit_wait",
                    kind = "direct",
                    upload_sequence = sequence,
                    attempt,
                    worker = worker_index,
                    rate_limit_wait_ms,
                    "performance.upload.rate_limit_wait"
                );
                let filename = telegram_camouflaged_filename();
                let last_reported = Arc::new(AtomicU64::new(0));
                let last_reported_net = Arc::new(AtomicU64::new(0));
                let last_for_cb = Arc::clone(&last_reported);
                let last_net_for_cb = Arc::clone(&last_reported_net);
                info!(
                    event = "performance.upload.start",
                    kind = "direct",
                    upload_sequence = sequence,
                    attempt,
                    worker = worker_index,
                    payload_bytes = bytes_len,
                    queue_wait_ms,
                    rate_limit_wait_ms,
                    "performance.upload.start"
                );
                let upload_started = Instant::now();
                let upload_res = storage
                    .upload_document_with_progress(
                        &filename,
                        blob.clone(),
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
                    .await;
                info!(
                    event = "performance.upload.finish",
                    kind = "direct",
                    upload_sequence = sequence,
                    attempt,
                    worker = worker_index,
                    payload_bytes = bytes_len,
                    rpc_duration_ms = upload_started.elapsed().as_millis() as u64,
                    result = if upload_res.is_ok() {
                        "succeeded"
                    } else {
                        "failed"
                    },
                    "performance.upload.finish"
                );

                match upload_res {
                    Ok(object_id) => {
                        let reported = last_reported.load(Ordering::Relaxed);
                        if reported < bytes_len {
                            uploaded_bytes.fetch_add(bytes_len - reported, Ordering::Relaxed);
                        }
                        return Ok(UploadOutcome::Direct {
                            chunk_hash,
                            object_id,
                            bytes: bytes_len,
                            source_bytes,
                        });
                    }
                    Err(e) => {
                        let reported = last_reported.load(Ordering::Relaxed).min(bytes_len);
                        if reported > 0 {
                            saturating_sub_u64(uploaded_bytes, reported);
                        }
                        let reported_net = last_reported_net.load(Ordering::Relaxed);
                        if reported_net > 0 {
                            saturating_sub_u64(uploaded_net_bytes, reported_net);
                        }

                        if attempt < UPLOAD_OBJECT_MAX_ATTEMPTS && is_retryable_upload_error(&e) {
                            let backoff = upload_object_retry_backoff(attempt);
                            warn!(
                                event = "io.telegram.upload_retry",
                                provider,
                                kind = "direct",
                                chunk_hash,
                                blob_bytes = bytes_len,
                                attempt,
                                max_attempts = UPLOAD_OBJECT_MAX_ATTEMPTS,
                                backoff_ms = backoff.as_millis() as u64,
                                error = %e,
                                "io.telegram.upload_retry"
                            );
                            let retry_wait_started = Instant::now();
                            sleep(backoff).await;
                            info!(
                                event = "performance.upload.retry_wait",
                                kind = "direct",
                                upload_sequence = sequence,
                                attempt,
                                worker = worker_index,
                                retry_wait_ms = retry_wait_started.elapsed().as_millis() as u64,
                                "performance.upload.retry_wait"
                            );
                            continue;
                        }

                        error!(
                            event = "io.telegram.upload_failed",
                            provider,
                            chunk_hash,
                            blob_bytes = bytes_len,
                            attempts = attempt,
                            error = %e,
                            "io.telegram.upload_failed"
                        );
                        return Err(Error::Telegram {
                            message: format!(
                                "upload failed: kind=direct chunk_hash={chunk_hash} bytes={bytes_len}; {e}"
                            ),
                        });
                    }
                }
            }

            Err(Error::Telegram {
                message: format!(
                    "upload failed: kind=direct chunk_hash={chunk_hash} bytes={bytes_len}; retry loop exhausted"
                ),
            })
        }
        UploadJob::Pack {
            sequence,
            queued_at,
            entries,
            pack_bytes,
            source_bytes,
            _bytes_permit,
        } => {
            let bytes_len = pack_bytes.len() as u64;
            let queue_wait_ms = queued_at.elapsed().as_millis() as u64;
            for attempt in 1..=UPLOAD_OBJECT_MAX_ATTEMPTS {
                let rate_limit_wait_ms = limiter.wait_turn().await.as_millis() as u64;
                info!(
                    event = "performance.upload.rate_limit_wait",
                    kind = "pack",
                    upload_sequence = sequence,
                    attempt,
                    worker = worker_index,
                    rate_limit_wait_ms,
                    "performance.upload.rate_limit_wait"
                );
                let filename = telegram_camouflaged_filename();
                let last_reported = Arc::new(AtomicU64::new(0));
                let last_reported_net = Arc::new(AtomicU64::new(0));
                let last_for_cb = Arc::clone(&last_reported);
                let last_net_for_cb = Arc::clone(&last_reported_net);
                info!(
                    event = "performance.upload.start",
                    kind = "pack",
                    upload_sequence = sequence,
                    attempt,
                    worker = worker_index,
                    payload_bytes = bytes_len,
                    queue_wait_ms,
                    rate_limit_wait_ms,
                    "performance.upload.start"
                );
                let upload_started = Instant::now();
                let upload_res = storage
                    .upload_document_with_progress(
                        &filename,
                        pack_bytes.clone(),
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
                    .await;
                info!(
                    event = "performance.upload.finish",
                    kind = "pack",
                    upload_sequence = sequence,
                    attempt,
                    worker = worker_index,
                    payload_bytes = bytes_len,
                    rpc_duration_ms = upload_started.elapsed().as_millis() as u64,
                    result = if upload_res.is_ok() {
                        "succeeded"
                    } else {
                        "failed"
                    },
                    "performance.upload.finish"
                );

                match upload_res {
                    Ok(pack_object_id) => {
                        let reported = last_reported.load(Ordering::Relaxed);
                        if reported < bytes_len {
                            uploaded_bytes.fetch_add(bytes_len - reported, Ordering::Relaxed);
                        }
                        return Ok(UploadOutcome::Pack {
                            entries,
                            pack_object_id,
                            bytes: bytes_len,
                            source_bytes,
                        });
                    }
                    Err(e) => {
                        let reported = last_reported.load(Ordering::Relaxed).min(bytes_len);
                        if reported > 0 {
                            saturating_sub_u64(uploaded_bytes, reported);
                        }
                        let reported_net = last_reported_net.load(Ordering::Relaxed);
                        if reported_net > 0 {
                            saturating_sub_u64(uploaded_net_bytes, reported_net);
                        }

                        if attempt < UPLOAD_OBJECT_MAX_ATTEMPTS && is_retryable_upload_error(&e) {
                            let backoff = upload_object_retry_backoff(attempt);
                            warn!(
                                event = "io.telegram.upload_retry",
                                provider,
                                kind = "pack",
                                blob_bytes = bytes_len,
                                attempt,
                                max_attempts = UPLOAD_OBJECT_MAX_ATTEMPTS,
                                backoff_ms = backoff.as_millis() as u64,
                                error = %e,
                                "io.telegram.upload_retry"
                            );
                            let retry_wait_started = Instant::now();
                            sleep(backoff).await;
                            info!(
                                event = "performance.upload.retry_wait",
                                kind = "pack",
                                upload_sequence = sequence,
                                attempt,
                                worker = worker_index,
                                retry_wait_ms = retry_wait_started.elapsed().as_millis() as u64,
                                "performance.upload.retry_wait"
                            );
                            continue;
                        }

                        error!(
                            event = "io.telegram.upload_failed",
                            provider,
                            blob_bytes = bytes_len,
                            attempts = attempt,
                            error = %e,
                            "io.telegram.upload_failed"
                        );
                        return Err(Error::Telegram {
                            message: format!("upload failed: kind=pack bytes={bytes_len}; {e}"),
                        });
                    }
                }
            }

            Err(Error::Telegram {
                message: format!(
                    "upload failed: kind=pack bytes={bytes_len}; retry loop exhausted"
                ),
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
        db_path = %config.endpoint_db_path.display(),
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
    // Treat `rate_limit.max_concurrent_uploads` as a hard cap. Adaptive mode may downshift on
    // FloodWait, but should never exceed the configured maximum.
    let adaptive_max_concurrency = configured_concurrency;
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
    let scan_source_files_done = Arc::new(AtomicU64::new(0));
    let scan_chunks_total = Arc::new(AtomicU64::new(0));
    let scan_bytes_read = Arc::new(AtomicU64::new(0));
    let scan_bytes_deduped = Arc::new(AtomicU64::new(0));
    let scan_source_bytes_need_upload = Arc::new(AtomicU64::new(0));
    let uploaded_source_bytes = Arc::new(AtomicU64::new(0));
    let uploaded_bytes = Arc::new(AtomicU64::new(0));
    let upload_workload_total = Arc::new(AtomicU64::new(0));
    let upload_confirmed_bytes = Arc::new(AtomicU64::new(0));
    let uploaded_net_bytes = Arc::new(AtomicU64::new(0));
    let have_uploaded_net_bytes = Arc::new(AtomicBool::new(false));
    let scan_done = Arc::new(AtomicBool::new(false));
    let upload_phase_started = Arc::new(AtomicBool::new(false));
    let active_uploads = Arc::new(AtomicUsize::new(0));
    let pending_jobs = Arc::new(AtomicUsize::new(0));
    let pending_bytes = Arc::new(AtomicU64::new(0));
    let upload_sequence = Arc::new(AtomicU64::new(1));
    let queue_wait_sequence = Arc::new(AtomicU64::new(1));
    let scan_queue_blocked_ms = Arc::new(AtomicU64::new(0));

    let scan_source_path = config.source_path.clone();
    let snapshot_id = config
        .snapshot_id
        .clone()
        .unwrap_or_else(|| format!("snp_{}", uuid::Uuid::new_v4()));
    let filemap_db_path = config.filemap_dir.join(format!("{snapshot_id}.sqlite"));
    let scan_label = config.label.clone();
    let scan_chunking = config.chunking.clone();
    let scan_master_key = config.master_key;
    let scan_endpoint_db_path = config.endpoint_db_path.clone();
    let scan_filemap_dir = config.filemap_dir.clone();
    let scan_filemap_db_path = filemap_db_path.clone();

    std::fs::create_dir_all(&config.filemap_dir)?;

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
        planned_upload_bytes: Arc::clone(&upload_workload_total),
        phase_started: Arc::clone(&upload_phase_started),
        next_sequence: Arc::clone(&upload_sequence),
        next_queue_wait_id: Arc::clone(&queue_wait_sequence),
        scan_queue_blocked_ms: Arc::clone(&scan_queue_blocked_ms),
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
                active_uploads.fetch_add(1, Ordering::Relaxed);
                let _token = ActiveUploadToken(active_uploads.as_ref());
                adaptive.on_attempt();
                let outcome = process_upload_job(
                    storage,
                    &provider,
                    _slot.worker_index,
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
    let pool = open_index_db(&config.endpoint_db_path).await?;
    let mut conn: DbConn = pool.acquire().await?;
    drop(pool);

    let dedupe_enabled = config.remote_dedupe.enabled();
    let mut dedupe_conn: Option<DbConn> = None;
    if dedupe_enabled {
        if let Some(parent) = config.dedupe_db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let pool = open_index_db(&config.dedupe_db_path).await?;
        let conn: DbConn = pool.acquire().await?;
        drop(pool);
        dedupe_conn = Some(conn);

        if let Some(parent) = config.dedupe_pending_db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Ensure the pending spool DB exists so upload checkpoints can append immediately.
        let _ = open_index_db(&config.dedupe_pending_db_path).await?;

        // First-time migration: if the local dedupe DB is empty, seed it from the legacy endpoint
        // DB so we don't lose cross-target dedupe on upgrade.
        if let Some(dedupe_conn) = dedupe_conn.as_mut()
            && !dedupe_db_has_any_chunk_objects(dedupe_conn).await?
        {
            seed_dedupe_db_from_endpoint_db(dedupe_conn, &config.endpoint_db_path).await?;
        }
    }

    // Retention must run before snapshot insertion so repeated failed runs do not keep inflating
    // the shared endpoint index DB beyond the configured window.
    //
    // NOTE: This used to run for *all* sources (apply_retention_all_sources). On large endpoints
    // (e.g. millions of files across multiple targets) that makes every backup pay an O(db-size)
    // maintenance cost before any scanning/upload begins, which can look like a "stuck" backup.
    // Restrict retention to the source being backed up; other sources will be cleaned up when
    // they run, or via an explicit maintenance task.
    let pruned_preflight =
        match apply_retention(&mut conn, &config.source_path, config.keep_last_snapshots).await {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    event = "snapshots.retention.preflight_failed",
                    source_path = %config.source_path.display(),
                    error = %e,
                    "snapshots.retention.preflight_failed"
                );
                Vec::new()
            }
        };
    cleanup_filemap_cache_best_effort(&config.filemap_dir, &pruned_preflight);
    compact_index_db_if_needed(&mut conn, &config.endpoint_db_path).await;

    let scan_future = {
        let conn = &mut conn;
        let dedupe_conn = &mut dedupe_conn;
        let uploader = uploader.clone();
        let upload_tx = upload_tx.clone();
        let upload_tx_for_error = upload_tx.clone();
        let cancel = upload_cancel.clone();
        let scan_files_indexed = Arc::clone(&scan_files_indexed);
        let scan_source_files_done = Arc::clone(&scan_source_files_done);
        let scan_chunks_total = Arc::clone(&scan_chunks_total);
        let scan_bytes_read = Arc::clone(&scan_bytes_read);
        let scan_bytes_deduped = Arc::clone(&scan_bytes_deduped);
        let scan_source_bytes_need_upload = Arc::clone(&scan_source_bytes_need_upload);
        let uploaded_source_bytes = Arc::clone(&uploaded_source_bytes);
        let uploaded_bytes = Arc::clone(&uploaded_bytes);
        let upload_workload_total = Arc::clone(&upload_workload_total);
        let upload_confirmed_bytes = Arc::clone(&upload_confirmed_bytes);
        let uploaded_net_bytes = Arc::clone(&uploaded_net_bytes);
        let have_uploaded_net_bytes = Arc::clone(&have_uploaded_net_bytes);
        let scan_done = Arc::clone(&scan_done);
        let upload_phase_started = Arc::clone(&upload_phase_started);
        let active_uploads = Arc::clone(&active_uploads);
        let scan_queue_blocked_ms = Arc::clone(&scan_queue_blocked_ms);
        async move {
            let performance_scan_started = Instant::now();
            let mut scan_performance = ScanPerformance::new(performance_scan_started);
            info!(
                event = "performance.scan.start",
                phase = "scan",
                "performance.scan.start"
            );
            let res = async {
                let sqlite_started = Instant::now();
                let base_snapshot_id =
                    latest_snapshot_for_source(conn, &scan_source_path, provider).await?;
                scan_performance.record(ScanWorkKind::Sqlite, sqlite_started);
                let snapshot_id = snapshot_id.clone();
                let source_path_utf8 = path_to_utf8(&scan_source_path)?;

                let sqlite_started = Instant::now();
                let (_, retry_waits) = execute_scan_sqlite_with_busy_retry!(
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
                scan_performance.record_sqlite("snapshots.insert", sqlite_started, &retry_waits);

                // Create per-snapshot filemap DB and seed the snapshot row. This DB is uploaded
                // as the snapshot's "filemap index", while the endpoint DB remains small and only
                // stores global/dedupe state.
                let filemap_pool = open_snapshot_filemap_db(&scan_filemap_db_path).await?;
                let mut filemap_conn: DbConn = filemap_pool.acquire().await?;
                drop(filemap_pool);
                let sqlite_started = Instant::now();
                let (_, retry_waits) = execute_scan_sqlite_with_busy_retry!(
                    "snapshots.insert.filemap",
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
                    .execute(&mut *filemap_conn)
                )?;
                scan_performance.record_sqlite(
                    "snapshots.insert.filemap",
                    sqlite_started,
                    &retry_waits,
                );

                // If we have a base snapshot, attach its filemap DB as `base` so base-chunk-copy
                // can copy `file_chunks` without re-chunking file contents.
                if let Some(base_snapshot_id) = base_snapshot_id.as_deref() {
                    let cached_path = scan_filemap_dir.join(format!("{base_snapshot_id}.sqlite"));
                    let cached_filemap_exists = cached_path.exists();
                    let endpoint_filemap_exists = if cached_filemap_exists {
                        false
                    } else {
                        let sqlite_started = Instant::now();
                        let has_filemap =
                            endpoint_db_has_snapshot_filemap(conn, base_snapshot_id).await?;
                        scan_performance.record(ScanWorkKind::Sqlite, sqlite_started);
                        has_filemap
                    };
                    let base_db_path = if cached_filemap_exists {
                        cached_path
                    } else if endpoint_filemap_exists {
                        // Upgrade/legacy path: endpoint DB might still contain the base snapshot's
                        // filemap rows.
                        scan_endpoint_db_path.clone()
                    } else {
                        let sqlite_started = Instant::now();
                        let manifest_object_id = lookup_remote_index_manifest_object_id(
                            conn,
                            base_snapshot_id,
                            provider,
                        )
                        .await?
                        .ok_or_else(|| Error::Integrity {
                            message: format!(
                                "base snapshot missing remote index pointer: base_snapshot_id={base_snapshot_id}"
                            ),
                        })?;
                        scan_performance.record(ScanWorkKind::Sqlite, sqlite_started);

                        crate::remote_index_db::download_and_write_index_db_atomic(
                            storage,
                            base_snapshot_id,
                            &manifest_object_id,
                            &scan_master_key,
                            &cached_path,
                            options.cancel,
                            Some(provider),
                            None,
                        )
                        .await?;
                        cached_path
                    };

                    let sqlite_started = Instant::now();
                    attach_db(&mut filemap_conn, "base", &base_db_path).await?;
                    scan_performance.record(ScanWorkKind::Sqlite, sqlite_started);
                }

                // Keep the per-snapshot filemap writes atomic for the whole scan. Individual
                // statements remain bounded to the scan batch constants below, while a single
                // commit avoids paying SQLite's durable commit cost once per 512-file batch.
                let mut filemap_scan_tx = filemap_conn.begin().await.map_err(Error::from)?;

                let mut result = BackupResult {
                    snapshot_id: snapshot_id.clone(),
                    ..BackupResult::default()
                };

                let global_conn: &mut DbConn = if dedupe_enabled {
                    dedupe_conn
                        .as_mut()
                        .ok_or_else(|| Error::InvalidConfig {
                            message: "dedupe_db_path is required when remote_dedupe is enabled"
                                .to_string(),
                        })?
                } else {
                    conn
                };
                let sqlite_started = Instant::now();
                let mut known_chunk_hashes =
                    load_chunk_hashes_for_storage(global_conn, storage, provider).await?;
                scan_performance.record(ScanWorkKind::Sqlite, sqlite_started);
                let mut pack_enabled = false;
                let mut pending_bytes: usize = 0;
                let mut pending_uploads: Vec<SourceBlob> = Vec::new();
                let mut pack_state = PackState::new(provider, &snapshot_id);
                let mut pending_base_chunk_copies: Vec<BaseFileChunkCopyRow> = Vec::new();
                let mut base_chunks_seeded = false;
                let mut base_copy_map_initialized = false;
                let mut warned_ignore_errors = HashSet::<String>::new();
                let mut seen_ignore_files = HashSet::<PathBuf>::new();
                let mut ignore_rule_files = 0u64;

                if let Some(sink) = options.progress {
                    sink.on_progress(TaskProgress {
                        phase: "scan".to_string(),
                        files_total: None,
                        files_done: Some(0),
                        source_files_total,
                        source_bytes_total,
                        source_bytes_need_upload_total: Some(0),
                        chunks_total: Some(0),
                        chunks_done: Some(0),
                        bytes_read: Some(0),
                        upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
                        bytes_uploaded_confirmed: Some(
                            upload_confirmed_bytes.load(Ordering::Relaxed),
                        ),
                        bytes_uploaded_source: Some(0),
                        bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
                        net_bytes_uploaded: have_uploaded_net_bytes
                            .load(Ordering::Relaxed)
                            .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
                        bytes_downloaded: None,
                        net_bytes_downloaded: None,
                        bytes_deduped: Some(0),
                    });
                }

                let mut source_walk = build_source_walk(&scan_source_path);
                loop {
                    let mut pending_scan_entries =
                        Vec::with_capacity(SCAN_FILE_METADATA_BATCH_SIZE);
                    while pending_scan_entries.len() < SCAN_FILE_METADATA_BATCH_SIZE {
                        let walk_started = Instant::now();
                        let Some(entry) = source_walk.next() else {
                            break;
                        };
                        scan_performance.record(ScanWorkKind::Walk, walk_started);
                        if let Some(cancel) = options.cancel
                            && cancel.is_cancelled()
                        {
                            return Err(Error::Cancelled);
                        }

                        let entry = match entry {
                            Ok(v) => v,
                            Err(e) => {
                                if ignore_error_is_rule_parse_only(&e) {
                                    warn_invalid_televyignore_rule_once(
                                        &mut warned_ignore_errors,
                                        &e,
                                        &scan_source_path,
                                        "scan",
                                    );
                                    continue;
                                }
                                if ignore_error_is_non_root_not_found(&e, &scan_source_path) {
                                    debug!(
                                        event = "scan.walkdir.not_found",
                                        error = %e,
                                        "scan.walkdir.not_found"
                                    );
                                    continue;
                                }
                                return Err(map_ignore_error(e, &scan_source_path));
                            }
                        };

                        if let Some(err) = entry.error() {
                            if ignore_error_is_rule_parse_only(err) {
                                warn_invalid_televyignore_rule_once(
                                    &mut warned_ignore_errors,
                                    err,
                                    &scan_source_path,
                                    "scan",
                                );
                            } else if ignore_error_is_not_found(err)
                                && entry.path() != scan_source_path
                            {
                                debug!(
                                    event = "scan.walkdir.not_found",
                                    path = %entry.path().display(),
                                    error = %err,
                                    "scan.walkdir.not_found"
                                );
                                continue;
                            } else {
                                return Err(map_ignore_error(err.clone(), &scan_source_path));
                            }
                        }

                        let path = entry.path();
                        let metadata_started = Instant::now();
                        let metadata_result = entry.metadata();
                        scan_performance.record(ScanWorkKind::Metadata, metadata_started);
                        let metadata = match metadata_result {
                            Ok(v) => v,
                            Err(e) => {
                                if ignore_error_is_not_found(&e) {
                                    debug!(
                                        event = "scan.entry_not_found",
                                        path = %path.display(),
                                        error = %e,
                                        "scan.entry_not_found"
                                    );
                                    continue;
                                }
                                return Err(map_ignore_error(e, &scan_source_path));
                            }
                        };

                        if metadata.is_dir() {
                            ignore_rule_files = ignore_rule_files.saturating_add(
                                count_ignore_file_for_dir(&mut seen_ignore_files, path),
                            );
                        }

                        if metadata.is_file()
                            && path.file_name() == Some(OsStr::new(TELEVYIGNORE_FILE_NAME))
                            && seen_ignore_files.insert(path.to_path_buf())
                        {
                            ignore_rule_files = ignore_rule_files.saturating_add(1);
                        }

                        if path == scan_source_path {
                            continue;
                        }

                        let rel_path = path.strip_prefix(&scan_source_path).map_err(|_| {
                            Error::InvalidConfig {
                                message: "path strip_prefix failed".to_string(),
                            }
                        })?;
                        let rel_path = path_to_utf8(rel_path)?;

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

                        pending_scan_entries.push(PendingScanEntry {
                            path: path.to_path_buf(),
                            rel_path,
                            kind,
                            size,
                            mtime_ms,
                            mode,
                            file_id: format!("f_{}", uuid::Uuid::new_v4()),
                        });
                    }

                    if pending_scan_entries.is_empty() {
                        break;
                    }

                    let sqlite_started = Instant::now();
                    let retry_waits = insert_scan_file_rows_batch_in_tx(
                        &mut filemap_scan_tx,
                        &snapshot_id,
                        &pending_scan_entries,
                    )
                    .await?;
                    scan_performance.record_sqlite("files.insert", sqlite_started, &retry_waits);

                    let mut base_rows = if let Some(base_snapshot_id) = base_snapshot_id.as_deref()
                        && pending_scan_entries.iter().any(|entry| entry.kind == "file")
                    {
                        let sqlite_started = Instant::now();
                        let (rows, retry_waits) = lookup_base_file_snapshot_rows_in_tx(
                            &mut filemap_scan_tx,
                            base_snapshot_id,
                            &pending_scan_entries,
                        )
                        .await?;
                        scan_performance.record_sqlite(
                            "base.files.lookup",
                            sqlite_started,
                            &retry_waits,
                        );
                        rows
                    } else {
                        HashMap::new()
                    };

                    for PendingScanEntry {
                        path,
                        rel_path,
                        kind,
                        size,
                        mtime_ms,
                        mode,
                        file_id,
                    } in pending_scan_entries
                    {
                        if let Some(cancel) = options.cancel
                            && cancel.is_cancelled()
                        {
                            return Err(Error::Cancelled);
                        }

                        result.files_total += 1;
                        result.files_indexed += 1;
                        scan_files_indexed.store(result.files_indexed, Ordering::Relaxed);

                        if kind == "file" {
                            scan_source_files_done.fetch_add(1, Ordering::Relaxed);
                        } else {
                            continue;
                        }

                        let mut copied_from_base = false;
                        let base_row = base_rows.remove(&rel_path);
                        if let Some(base_row) = base_row
                            && base_row.size == size
                            && base_row.mtime_ms == mtime_ms
                            && base_row.mode == mode
                        {
                            // Metadata was collected before the batch transaction and base lookup.
                            // Revalidate before copying old chunks so a transient or changed file
                            // is handled by the existing changed-file path instead.
                            let metadata_started = Instant::now();
                            let revalidation =
                                revalidate_file_for_base_copy(&path, size, mtime_ms, mode);
                            scan_performance.record(ScanWorkKind::Metadata, metadata_started);
                            match revalidation? {
                                BaseCopyRevalidation::NotFound => {
                                    let sqlite_started = Instant::now();
                                    let retry_waits =
                                        delete_transient_scan_file_in_tx(
                                            &mut filemap_scan_tx,
                                            &file_id,
                                        )
                                        .await?;
                                    scan_performance.record_sqlite(
                                        "files.transient_delete",
                                        sqlite_started,
                                        &retry_waits,
                                    );
                                    debug!(
                                        event = "scan.file_not_found",
                                        path = %path.display(),
                                        "scan.file_not_found"
                                    );
                                    continue;
                                }
                                BaseCopyRevalidation::Match => {
                                    if !base_chunks_seeded {
                                        if let Some(cancel) = options.cancel
                                            && cancel.is_cancelled()
                                        {
                                            return Err(Error::Cancelled);
                                        }
                                        let sqlite_started = Instant::now();
                                        let retry_waits = seed_base_snapshot_chunks_in_tx(
                                            &mut filemap_scan_tx,
                                            base_snapshot_id.as_deref().unwrap_or_default(),
                                        )
                                        .await?;
                                        scan_performance.record_sqlite(
                                            "base_copy",
                                            sqlite_started,
                                            &retry_waits,
                                        );
                                        base_chunks_seeded = true;
                                    }
                                    if !base_copy_map_initialized {
                                        let sqlite_started = Instant::now();
                                        let retry_waits = initialize_base_chunk_copy_map_in_tx(
                                            &mut filemap_scan_tx,
                                        )
                                        .await?;
                                        scan_performance.record_sqlite(
                                            "base_copy",
                                            sqlite_started,
                                            &retry_waits,
                                        );
                                        base_copy_map_initialized = true;
                                    }
                                    pending_base_chunk_copies.push(BaseFileChunkCopyRow {
                                        file_id: file_id.clone(),
                                        base_file_id: base_row.file_id,
                                        size: size.max(0) as u64,
                                    });
                                    copied_from_base = true;
                                }
                                BaseCopyRevalidation::Changed {
                                    size,
                                    mtime_ms,
                                    mode,
                                } => {
                                    let sqlite_started = Instant::now();
                                    let retry_waits = update_scan_file_metadata_in_tx(
                                        &mut filemap_scan_tx,
                                        &file_id,
                                        size,
                                        mtime_ms,
                                        mode,
                                    )
                                    .await?;
                                    scan_performance.record_sqlite(
                                        "files.metadata_update",
                                        sqlite_started,
                                        &retry_waits,
                                    );
                                }
                                BaseCopyRevalidation::NotFile => {
                                    let sqlite_started = Instant::now();
                                    let retry_waits =
                                        delete_transient_scan_file_in_tx(
                                            &mut filemap_scan_tx,
                                            &file_id,
                                        )
                                        .await?;
                                    scan_performance.record_sqlite(
                                        "files.transient_delete",
                                        sqlite_started,
                                        &retry_waits,
                                    );
                                    debug!(
                                        event = "scan.file_type_changed",
                                        path = %path.display(),
                                        "scan.file_type_changed"
                                    );
                                    continue;
                                }
                            }
                            if copied_from_base
                                && pending_base_chunk_copies.len() >= BASE_FILE_CHUNK_COPY_BATCH_SIZE
                            {
                                let deduped_bytes =
                                    base_copy_rows_bytes(&pending_base_chunk_copies);
                                let sqlite_started = Instant::now();
                                let retry_waits = stage_base_chunk_copy_batch_in_tx(
                                    &mut filemap_scan_tx,
                                    &mut pending_base_chunk_copies,
                                )
                                .await?;
                                scan_performance.record_sqlite(
                                    "base_copy",
                                    sqlite_started,
                                    &retry_waits,
                                );
                                result.bytes_deduped =
                                    result.bytes_deduped.saturating_add(deduped_bytes);
                                scan_bytes_deduped.store(result.bytes_deduped, Ordering::Relaxed);
                            }
                            if copied_from_base {
                                continue;
                            }
                        }

                        let file = match File::open(&path) {
                            Ok(f) => f,
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                                let sqlite_started = Instant::now();
                                let retry_waits =
                                    delete_transient_scan_file_in_tx(
                                        &mut filemap_scan_tx,
                                        &file_id,
                                    )
                                    .await?;
                                scan_performance.record_sqlite(
                                    "files.transient_delete",
                                    sqlite_started,
                                    &retry_waits,
                                );
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
                        let mut filemap_chunk_rows: Vec<FilemapChunkRow> = Vec::new();

                        let mut chunks = chunker.enumerate();
                        loop {
                            let read_chunk_started = Instant::now();
                            let Some((seq, chunk)) = chunks.next() else {
                                break;
                            };
                            scan_performance.record(ScanWorkKind::ReadChunk, read_chunk_started);
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

                            let hash_started = Instant::now();
                            let chunk_hash = blake3::hash(&chunk.data).to_hex().to_string();
                            scan_performance.record(ScanWorkKind::Hash, hash_started);

                            // `file_chunks` has a FK to `chunks`; defer the filemap chunk-row
                            // insert until this file is fully chunked, then write bounded multi-row
                            // statements before materializing its file_chunks rows.
                            filemap_chunk_rows.push(FilemapChunkRow {
                                chunk_hash: chunk_hash.clone(),
                                size: chunk.data.len() as i64,
                            });

                            let exists = known_chunk_hashes.contains(&chunk_hash);
                            if exists {
                                result.bytes_deduped += chunk.data.len() as u64;
                                scan_bytes_deduped.store(result.bytes_deduped, Ordering::Relaxed);
                            } else {
                                known_chunk_hashes.insert(chunk_hash.clone());
                                scan_source_bytes_need_upload
                                    .fetch_add(chunk.data.len() as u64, Ordering::Relaxed);

                                let sqlite_started = Instant::now();
                                let (_, retry_waits) = execute_scan_sqlite_with_busy_retry!(
                                    "chunks.insert",
                                    sqlx::query(
                                        r#"
                                        INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
                                        VALUES (?, ?, 'blake3', 'xchacha20poly1305', strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                                        "#,
                                    )
                                    .bind(&chunk_hash)
                                    .bind(chunk.data.len() as i64)
                                    .execute(&mut **global_conn)
                                )?;
                                scan_performance.record_sqlite(
                                    "chunks.insert",
                                    sqlite_started,
                                    &retry_waits,
                                );

                                let encrypt_started = Instant::now();
                                let encrypted = encrypt_framed(
                                    &scan_master_key,
                                    chunk_hash.as_bytes(),
                                    &chunk.data,
                                )?;
                                scan_performance.record(ScanWorkKind::Encrypt, encrypt_started);
                                let blob = SourceBlob {
                                    chunk_hash: chunk_hash.clone(),
                                    blob: encrypted,
                                    source_bytes: chunk.data.len() as u64,
                                };
                                if !pack_enabled {
                                    pending_bytes = pending_bytes.saturating_add(blob.blob.len());
                                    pending_uploads.push(blob);
                                    if pending_uploads.len() > PACK_ENABLE_MIN_OBJECTS
                                        || pending_bytes > PACK_TARGET_BYTES
                                    {
                                        pack_enabled = true;
                                        for b in pending_uploads.drain(..) {
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

                                if pack_enabled {
                                    let should_flush_for_progress =
                                        active_uploads.load(Ordering::Relaxed) == 0
                                            && pack_state.packer.entries_len()
                                                >= PACK_ENABLE_MIN_OBJECTS;
                                    if should_flush_for_progress
                                        || pack_state.should_flush_due_to_age()
                                    {
                                        flush_packer(&uploader, &scan_master_key, &mut pack_state)
                                            .await?;
                                    }
                                }
                            }

                            file_chunk_rows.push(FileChunkRow {
                                seq: seq as i64,
                                chunk_hash,
                                offset: chunk.offset as i64,
                                len: chunk.length as i64,
                            });
                        }

                        if !filemap_chunk_rows.is_empty() {
                            let sqlite_started = Instant::now();
                            let retry_waits = insert_filemap_chunks_batch_in_tx(
                                &mut filemap_scan_tx,
                                &filemap_chunk_rows,
                            )
                            .await?;
                            scan_performance.record_sqlite(
                                "chunks.insert.filemap",
                                sqlite_started,
                                &retry_waits,
                            );
                        }

                        if !file_chunk_rows.is_empty() {
                            let sqlite_started = Instant::now();
                            let retry_waits = insert_file_chunks_batch_in_tx(
                                &mut filemap_scan_tx,
                                &file_id,
                                &file_chunk_rows,
                            )
                            .await?;
                            scan_performance.record_sqlite(
                                "file_chunks.insert",
                                sqlite_started,
                                &retry_waits,
                            );
                        }
                    }
                }

                if base_copy_map_initialized
                    && let Some(cancel) = options.cancel
                    && cancel.is_cancelled()
                {
                    return Err(Error::Cancelled);
                }
                if !pending_base_chunk_copies.is_empty() {
                    let deduped_bytes = base_copy_rows_bytes(&pending_base_chunk_copies);
                    let sqlite_started = Instant::now();
                    let retry_waits = stage_base_chunk_copy_batch_in_tx(
                        &mut filemap_scan_tx,
                        &mut pending_base_chunk_copies,
                    )
                    .await?;
                    scan_performance.record_sqlite(
                        "base_copy",
                        sqlite_started,
                        &retry_waits,
                    );
                    result.bytes_deduped = result.bytes_deduped.saturating_add(deduped_bytes);
                    scan_bytes_deduped.store(result.bytes_deduped, Ordering::Relaxed);
                }
                if base_copy_map_initialized {
                    let sqlite_started = Instant::now();
                    let (copied_chunks, retry_waits) =
                        materialize_base_chunk_copy_map_in_tx(&mut filemap_scan_tx).await?;
                    scan_performance.record_sqlite(
                        "base_copy",
                        sqlite_started,
                        &retry_waits,
                    );
                    if copied_chunks > 0 {
                        result.chunks_total = result.chunks_total.saturating_add(copied_chunks);
                        scan_chunks_total.store(result.chunks_total, Ordering::Relaxed);
                    }
                }

                let sqlite_started = Instant::now();
                let commit_result = filemap_scan_tx.commit().await;
                scan_performance.record_sqlite("filemap.commit", sqlite_started, &[]);
                commit_result.map_err(Error::from)?;

                if pack_enabled {
                    flush_packer(&uploader, &scan_master_key, &mut pack_state).await?;
                } else {
                    for blob in pending_uploads {
                        uploader
                            .enqueue_direct(blob.chunk_hash, blob.blob, blob.source_bytes)
                            .await?;
                    }
                }

                let filemap_checkpoint_started = Instant::now();
                match checkpoint_snapshot_filemap_wal(&mut filemap_conn).await {
                    Ok((wal_log_frames, wal_checkpointed_frames)) => {
                        info!(
                            event = "performance.index.filemap_checkpoint",
                            phase = "scan",
                            duration_ms = filemap_checkpoint_started.elapsed().as_millis() as u64,
                            result = "succeeded",
                            wal_log_frames,
                            wal_checkpointed_frames,
                            "performance.index.filemap_checkpoint"
                        );
                    }
                    Err(error) => {
                        error!(
                            event = "performance.index.filemap_checkpoint",
                            phase = "scan",
                            duration_ms = filemap_checkpoint_started.elapsed().as_millis() as u64,
                            result = "failed",
                            error = %error,
                            "performance.index.filemap_checkpoint"
                        );
                        return Err(error);
                    }
                }

                result.ignore_rule_files = ignore_rule_files;
                result.ignore_invalid_rules = warned_ignore_errors.len() as u64;
                if result.ignore_invalid_rules > 0 {
                    warn!(
                        event = "source.ignore.summary",
                        phase = "scan",
                        source_path = %scan_source_path.display(),
                        ignore_file = TELEVYIGNORE_FILE_NAME,
                        ignore_rule_files = result.ignore_rule_files,
                        ignore_invalid_rules = result.ignore_invalid_rules,
                        "source.ignore.summary"
                    );
                } else {
                    info!(
                        event = "source.ignore.summary",
                        phase = "scan",
                        source_path = %scan_source_path.display(),
                        ignore_file = TELEVYIGNORE_FILE_NAME,
                        ignore_rule_files = result.ignore_rule_files,
                        ignore_invalid_rules = result.ignore_invalid_rules,
                        "source.ignore.summary"
                    );
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
                if !upload_phase_started.load(Ordering::Relaxed) {
                    debug!(event = "phase.start", phase = "upload", "phase.start");
                    upload_phase_started.store(true, Ordering::Relaxed);
                    if let Some(sink) = options.progress {
                        sink.on_progress(TaskProgress {
                            phase: "upload".to_string(),
                            files_total: None,
                            files_done: Some(if source_files_total.is_some() {
                                scan_source_files_done.load(Ordering::Relaxed)
                            } else {
                                scan_files_indexed.load(Ordering::Relaxed)
                            }),
                            source_files_total,
                            source_bytes_total,
                            source_bytes_need_upload_total: Some(
                                scan_source_bytes_need_upload.load(Ordering::Relaxed),
                            ),
                            chunks_total: Some(scan_chunks_total.load(Ordering::Relaxed)),
                            chunks_done: Some(scan_chunks_total.load(Ordering::Relaxed)),
                            bytes_read: Some(scan_bytes_read.load(Ordering::Relaxed)),
                            upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
                            bytes_uploaded_confirmed: Some(
                                upload_confirmed_bytes.load(Ordering::Relaxed),
                            ),
                            bytes_uploaded_source: Some(
                                uploaded_source_bytes.load(Ordering::Relaxed),
                            ),
                            bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
                            bytes_downloaded: None,
                            net_bytes_uploaded: have_uploaded_net_bytes
                                .load(Ordering::Relaxed)
                                .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
                            net_bytes_downloaded: None,
                            bytes_deduped: Some(scan_bytes_deduped.load(Ordering::Relaxed)),
                        });
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
            scan_performance.upload_queue_blocked_ms =
                scan_queue_blocked_ms.load(Ordering::Relaxed);
            let scan_duration = performance_scan_started.elapsed();
            let (trace_json, trace_resolution_ms) = scan_performance.trace_json();
            info!(
                event = "performance.scan.trace",
                phase = "scan",
                trace_version = 2_u8,
                resolution_ms = trace_resolution_ms,
                trace_json = %trace_json,
                "performance.scan.trace"
            );
            match &res {
                Ok((_, result, _)) => {
                    info!(
                        event = "performance.scan.finish",
                        phase = "scan",
                        scan_duration_ms = scan_duration.as_millis() as u64,
                        walk_ms = scan_performance.walk_us / 1_000,
                        metadata_ms = scan_performance.metadata_us / 1_000,
                        read_chunk_ms = scan_performance.read_chunk_us / 1_000,
                        hash_ms = scan_performance.hash_us / 1_000,
                        encrypt_ms = scan_performance.encrypt_us / 1_000,
                        sqlite_timed_ms = scan_performance.sqlite_timed_us / 1_000,
                        sqlite_retry_wait_ms = scan_performance.sqlite_retry_wait_us / 1_000,
                        upload_queue_blocked_ms = scan_performance.upload_queue_blocked_ms,
                        unattributed_ms = (scan_duration.as_micros() as u64)
                            .saturating_sub(scan_performance.attributed_us())
                            / 1_000,
                        files_indexed = result.files_indexed,
                        chunks_total = result.chunks_total,
                        bytes_read = result.bytes_read,
                        result = "succeeded",
                        "performance.scan.finish"
                    );
                }
                Err(_) => {
                    info!(
                        event = "performance.scan.finish",
                        phase = "scan",
                        scan_duration_ms = scan_duration.as_millis() as u64,
                        walk_ms = scan_performance.walk_us / 1_000,
                        metadata_ms = scan_performance.metadata_us / 1_000,
                        read_chunk_ms = scan_performance.read_chunk_us / 1_000,
                        hash_ms = scan_performance.hash_us / 1_000,
                        encrypt_ms = scan_performance.encrypt_us / 1_000,
                        sqlite_timed_ms = scan_performance.sqlite_timed_us / 1_000,
                        sqlite_retry_wait_ms = scan_performance.sqlite_retry_wait_us / 1_000,
                        upload_queue_blocked_ms = scan_performance.upload_queue_blocked_ms,
                        unattributed_ms = (scan_duration.as_micros() as u64)
                            .saturating_sub(scan_performance.attributed_us())
                            / 1_000,
                        result = "failed",
                        "performance.scan.finish"
                    );
                }
            }
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
        let db_path_for_checkpoint = config.endpoint_db_path.clone();
        let dedupe_db_path_for_checkpoint = config.dedupe_db_path.clone();
        let pending_dedupe_db_path_for_checkpoint = config.dedupe_pending_db_path.clone();
        let provider_for_checkpoint = provider_owned.clone();
        let remote_dedupe_for_checkpoint = config.remote_dedupe.clone();
        let scan_files_indexed = Arc::clone(&scan_files_indexed);
        let scan_source_files_done = Arc::clone(&scan_source_files_done);
        let scan_chunks_total = Arc::clone(&scan_chunks_total);
        let scan_bytes_read = Arc::clone(&scan_bytes_read);
        let scan_bytes_deduped = Arc::clone(&scan_bytes_deduped);
        let scan_source_bytes_need_upload = Arc::clone(&scan_source_bytes_need_upload);
        let uploaded_source_bytes = Arc::clone(&uploaded_source_bytes);
        let uploaded_bytes = Arc::clone(&uploaded_bytes);
        let upload_workload_total = Arc::clone(&upload_workload_total);
        let upload_confirmed_bytes = Arc::clone(&upload_confirmed_bytes);
        let uploaded_net_bytes = Arc::clone(&uploaded_net_bytes);
        let have_uploaded_net_bytes = Arc::clone(&have_uploaded_net_bytes);
        let scan_done = Arc::clone(&scan_done);
        async move {
            let mut stats = UploadStats::default();
            let mut checkpoint_pool: Option<sqlx::SqlitePool> = None;
            let mut checkpoint_conn: Option<DbConn> = None;
            let mut pending_pool: Option<sqlx::SqlitePool> = None;
            let mut pending_conn: Option<DbConn> = None;
            let mut checkpoint_disabled = false;
            let mut rx = result_rx;
            while let Some(outcome) = rx.recv().await {
                match outcome {
                    Ok(UploadOutcome::Direct {
                        chunk_hash,
                        object_id,
                        bytes,
                        source_bytes,
                    }) => {
                        stats.chunk_objects.push(ChunkObjectMapping {
                            chunk_hash,
                            object_id: encode_tgfile_object_id(&object_id),
                            source_bytes,
                        });
                        stats.chunks_uploaded += 1;
                        stats.data_objects_uploaded += 1;
                        stats.bytes_uploaded += bytes;
                        upload_confirmed_bytes.fetch_add(bytes, Ordering::Relaxed);
                        uploaded_source_bytes.fetch_add(source_bytes, Ordering::Relaxed);
                    }
                    Ok(UploadOutcome::Pack {
                        entries,
                        pack_object_id,
                        bytes,
                        source_bytes,
                    }) => {
                        for entry in entries {
                            stats.chunk_objects.push(ChunkObjectMapping {
                                chunk_hash: entry.chunk_hash,
                                object_id: encode_tgpack_object_id(
                                    &pack_object_id,
                                    entry.offset,
                                    entry.len,
                                ),
                                source_bytes: entry.source_bytes,
                            });
                            stats.chunks_uploaded += 1;
                        }
                        stats.data_objects_uploaded += 1;
                        stats.bytes_uploaded += bytes;
                        upload_confirmed_bytes.fetch_add(bytes, Ordering::Relaxed);
                        uploaded_source_bytes.fetch_add(source_bytes, Ordering::Relaxed);
                    }
                    Err(e) => {
                        if stats.first_error.is_none() {
                            stats.first_error = Some(e);
                        }
                    }
                }

                if !checkpoint_disabled
                    && stats.chunk_objects.len() >= CHUNK_OBJECT_CHECKPOINT_BATCH_SIZE
                {
                    if checkpoint_conn.is_none() {
                        let path = if remote_dedupe_for_checkpoint.enabled() {
                            &dedupe_db_path_for_checkpoint
                        } else {
                            &db_path_for_checkpoint
                        };
                        match open_existing_index_db(path).await {
                            Ok(pool) => match pool.acquire().await {
                                Ok(conn) => {
                                    checkpoint_pool = Some(pool);
                                    checkpoint_conn = Some(conn);
                                }
                                Err(e) => {
                                    warn!(
                                        event = "upload.checkpoint.open_failed",
                                        error = %e,
                                        "upload.checkpoint.open_failed"
                                    );
                                    checkpoint_disabled = true;
                                }
                            },
                            Err(e) => {
                                warn!(
                                    event = "upload.checkpoint.open_failed",
                                    error = %e,
                                    "upload.checkpoint.open_failed"
                                );
                                checkpoint_disabled = true;
                            }
                        }
                    }

                    if remote_dedupe_for_checkpoint.enabled() {
                        if pending_conn.is_none() {
                            match open_existing_index_db(&pending_dedupe_db_path_for_checkpoint)
                                .await
                            {
                                Ok(pool) => match pool.acquire().await {
                                    Ok(conn) => {
                                        pending_pool = Some(pool);
                                        pending_conn = Some(conn);
                                    }
                                    Err(e) => {
                                        warn!(
                                            event = "upload.checkpoint.open_failed",
                                            error = %e,
                                            "upload.checkpoint.open_failed"
                                        );
                                        checkpoint_disabled = true;
                                    }
                                },
                                Err(e) => {
                                    warn!(
                                        event = "upload.checkpoint.open_failed",
                                        error = %e,
                                        "upload.checkpoint.open_failed"
                                    );
                                    checkpoint_disabled = true;
                                }
                            }
                        }

                        if let (Some(dedupe), Some(pending)) =
                            (checkpoint_conn.as_mut(), pending_conn.as_mut())
                            && let Err(e) = record_dedupe_chunk_objects_batch(
                                dedupe,
                                pending,
                                &provider_for_checkpoint,
                                &stats.chunk_objects,
                            )
                            .await
                        {
                            warn!(
                                event = "upload.checkpoint.persist_failed",
                                error = %e,
                                "upload.checkpoint.persist_failed"
                            );
                            checkpoint_disabled = true;
                            checkpoint_conn = None;
                            checkpoint_pool = None;
                            pending_conn = None;
                            pending_pool = None;
                        } else if checkpoint_conn.is_some() && pending_conn.is_some() {
                            stats.chunk_objects.clear();
                        }
                    } else if let Some(conn) = checkpoint_conn.as_mut()
                        && let Err(e) = record_chunk_objects_batch(
                            conn,
                            &provider_for_checkpoint,
                            &stats.chunk_objects,
                        )
                        .await
                    {
                        warn!(
                            event = "upload.checkpoint.persist_failed",
                            error = %e,
                            "upload.checkpoint.persist_failed"
                        );
                        checkpoint_disabled = true;
                        checkpoint_conn = None;
                        checkpoint_pool = None;
                    } else if checkpoint_conn.is_some() {
                        stats.chunk_objects.clear();
                    }
                }

                if let Some(sink) = options.progress {
                    let phase = if scan_done.load(Ordering::Relaxed) {
                        "upload"
                    } else {
                        "scan_upload"
                    };
                    sink.on_progress(TaskProgress {
                        phase: phase.to_string(),
                        files_total: None,
                        files_done: Some(if source_files_total.is_some() {
                            scan_source_files_done.load(Ordering::Relaxed)
                        } else {
                            scan_files_indexed.load(Ordering::Relaxed)
                        }),
                        source_files_total,
                        source_bytes_total,
                        source_bytes_need_upload_total: Some(
                            scan_source_bytes_need_upload.load(Ordering::Relaxed),
                        ),
                        chunks_total: Some(scan_chunks_total.load(Ordering::Relaxed)),
                        chunks_done: Some(scan_chunks_total.load(Ordering::Relaxed)),
                        bytes_read: Some(scan_bytes_read.load(Ordering::Relaxed)),
                        upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
                        bytes_uploaded_confirmed: Some(
                            upload_confirmed_bytes.load(Ordering::Relaxed),
                        ),
                        bytes_uploaded_source: Some(uploaded_source_bytes.load(Ordering::Relaxed)),
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

            if !checkpoint_disabled && !stats.chunk_objects.is_empty() {
                if checkpoint_conn.is_none() {
                    let path = if remote_dedupe_for_checkpoint.enabled() {
                        &dedupe_db_path_for_checkpoint
                    } else {
                        &db_path_for_checkpoint
                    };
                    match open_existing_index_db(path).await {
                        Ok(pool) => match pool.acquire().await {
                            Ok(conn) => {
                                checkpoint_pool = Some(pool);
                                checkpoint_conn = Some(conn);
                            }
                            Err(e) => {
                                warn!(
                                    event = "upload.checkpoint.open_failed",
                                    error = %e,
                                    "upload.checkpoint.open_failed"
                                );
                            }
                        },
                        Err(e) => {
                            warn!(
                                event = "upload.checkpoint.open_failed",
                                error = %e,
                                "upload.checkpoint.open_failed"
                            );
                        }
                    }
                }

                if remote_dedupe_for_checkpoint.enabled() {
                    if pending_conn.is_none() {
                        match open_existing_index_db(&pending_dedupe_db_path_for_checkpoint).await {
                            Ok(pool) => match pool.acquire().await {
                                Ok(conn) => {
                                    pending_pool = Some(pool);
                                    pending_conn = Some(conn);
                                }
                                Err(e) => {
                                    warn!(
                                        event = "upload.checkpoint.open_failed",
                                        error = %e,
                                        "upload.checkpoint.open_failed"
                                    );
                                }
                            },
                            Err(e) => {
                                warn!(
                                    event = "upload.checkpoint.open_failed",
                                    error = %e,
                                    "upload.checkpoint.open_failed"
                                );
                            }
                        }
                    }

                    if let (Some(dedupe), Some(pending)) =
                        (checkpoint_conn.as_mut(), pending_conn.as_mut())
                        && let Err(e) = record_dedupe_chunk_objects_batch(
                            dedupe,
                            pending,
                            &provider_for_checkpoint,
                            &stats.chunk_objects,
                        )
                        .await
                    {
                        warn!(
                            event = "upload.checkpoint.persist_failed",
                            error = %e,
                            "upload.checkpoint.persist_failed"
                        );
                        checkpoint_conn = None;
                        checkpoint_pool = None;
                        pending_conn = None;
                        pending_pool = None;
                    } else if checkpoint_conn.is_some() && pending_conn.is_some() {
                        stats.chunk_objects.clear();
                    }
                } else if let Some(conn) = checkpoint_conn.as_mut()
                    && let Err(e) = record_chunk_objects_batch(
                        conn,
                        &provider_for_checkpoint,
                        &stats.chunk_objects,
                    )
                    .await
                {
                    warn!(
                        event = "upload.checkpoint.persist_failed",
                        error = %e,
                        "upload.checkpoint.persist_failed"
                    );
                    checkpoint_conn = None;
                    checkpoint_pool = None;
                } else if checkpoint_conn.is_some() {
                    stats.chunk_objects.clear();
                }
            }

            drop(checkpoint_conn);
            drop(checkpoint_pool);
            drop(pending_conn);
            drop(pending_pool);

            Ok::<UploadStats, Error>(stats)
        }
    };

    let workers_future = async { while workers.next().await.is_some() {} };

    let progress_future = {
        let cancel = upload_cancel.clone();
        let scan_done = Arc::clone(&scan_done);
        let upload_phase_started = Arc::clone(&upload_phase_started);
        let active_uploads = Arc::clone(&active_uploads);
        let pending_jobs = Arc::clone(&pending_jobs);
        let scan_files_indexed = Arc::clone(&scan_files_indexed);
        let scan_source_files_done = Arc::clone(&scan_source_files_done);
        let scan_chunks_total = Arc::clone(&scan_chunks_total);
        let scan_bytes_read = Arc::clone(&scan_bytes_read);
        let scan_bytes_deduped = Arc::clone(&scan_bytes_deduped);
        let scan_source_bytes_need_upload = Arc::clone(&scan_source_bytes_need_upload);
        let uploaded_source_bytes = Arc::clone(&uploaded_source_bytes);
        let uploaded_bytes = Arc::clone(&uploaded_bytes);
        let upload_workload_total = Arc::clone(&upload_workload_total);
        let upload_confirmed_bytes = Arc::clone(&upload_confirmed_bytes);
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
                    } else if upload_phase_started.load(Ordering::Relaxed) {
                        "scan_upload"
                    } else {
                        "scan"
                    };
                    sink.on_progress(TaskProgress {
                        phase: phase.to_string(),
                        files_total: None,
                        files_done: Some(if source_files_total.is_some() {
                            scan_source_files_done.load(Ordering::Relaxed)
                        } else {
                            scan_files_indexed.load(Ordering::Relaxed)
                        }),
                        source_files_total,
                        source_bytes_total,
                        source_bytes_need_upload_total: Some(
                            scan_source_bytes_need_upload.load(Ordering::Relaxed),
                        ),
                        chunks_total: Some(scan_chunks_total.load(Ordering::Relaxed)),
                        chunks_done: Some(scan_chunks_total.load(Ordering::Relaxed)),
                        bytes_read: Some(scan_bytes_read.load(Ordering::Relaxed)),
                        upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
                        bytes_uploaded_confirmed: Some(
                            upload_confirmed_bytes.load(Ordering::Relaxed),
                        ),
                        bytes_uploaded_source: Some(uploaded_source_bytes.load(Ordering::Relaxed)),
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

    let tail_res = if dedupe_enabled {
        let dedupe_conn = dedupe_conn.as_mut().ok_or_else(|| Error::InvalidConfig {
            message: "dedupe_db_path is required when remote_dedupe is enabled".to_string(),
        })?;

        let pool = open_existing_index_db(&config.dedupe_pending_db_path).await?;
        let mut pending_conn: DbConn = pool.acquire().await?;
        drop(pool);

        record_dedupe_chunk_objects_batch(
            dedupe_conn,
            &mut pending_conn,
            &provider_owned,
            &chunk_objects,
        )
        .await
    } else {
        record_chunk_objects_batch(&mut conn, &provider_owned, &chunk_objects).await
    };

    if let Err(tail_err) = tail_res {
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
            files_done: Some(if source_files_total.is_some() {
                scan_source_files_done.load(Ordering::Relaxed)
            } else {
                result.files_indexed
            }),
            source_files_total,
            source_bytes_total,
            source_bytes_need_upload_total: Some(
                scan_source_bytes_need_upload.load(Ordering::Relaxed),
            ),
            chunks_total: Some(result.chunks_total),
            chunks_done: Some(result.chunks_total),
            bytes_read: Some(result.bytes_read),
            upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
            bytes_uploaded_confirmed: Some(upload_confirmed_bytes.load(Ordering::Relaxed)),
            bytes_uploaded_source: Some(uploaded_source_bytes.load(Ordering::Relaxed)),
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
    let files_done_for_progress = if source_files_total.is_some() {
        scan_source_files_done.load(Ordering::Relaxed)
    } else {
        scan_files_indexed.load(Ordering::Relaxed)
    };
    let endpoint_temp_parent = config
        .endpoint_db_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let filemap_temp_parent = filemap_db_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);

    // 1) Upload per-snapshot filemap DB, then persist its manifest pointer in the endpoint DB.
    let uploaded_filemap = upload_index_sqlite_db(
        storage,
        &config,
        &snapshot_id,
        &filemap_db_path,
        &filemap_temp_parent,
        &adaptive_controller,
        &upload_cancel,
        &rate_limiter,
        uploaded_bytes.as_ref(),
        uploaded_net_bytes.as_ref(),
        have_uploaded_net_bytes.as_ref(),
        upload_workload_total.as_ref(),
        upload_confirmed_bytes.as_ref(),
        options.progress,
        source_files_total,
        source_bytes_total,
        files_done_for_progress,
        scan_chunks_total.load(Ordering::Relaxed),
        scan_bytes_read.load(Ordering::Relaxed),
        uploaded_source_bytes.load(Ordering::Relaxed),
        scan_source_bytes_need_upload.load(Ordering::Relaxed),
        scan_bytes_deduped.load(Ordering::Relaxed),
    )
    .await?;
    persist_snapshot_remote_index_meta(&mut conn, provider, &snapshot_id, &uploaded_filemap)
        .await?;

    // Apply retention now so the exported endpoint DB reflects the configured window.
    let pruned_final =
        match apply_retention(&mut conn, &config.source_path, config.keep_last_snapshots).await {
            Ok(v) => v,
            Err(e) => {
                warn!(
                    event = "snapshots.retention.final_failed",
                    source_path = %config.source_path.display(),
                    error = %e,
                    "snapshots.retention.final_failed"
                );
                Vec::new()
            }
        };
    cleanup_filemap_cache_best_effort(&config.filemap_dir, &pruned_final);

    // 2) Export+upload small endpoint DB (global/dedupe state, no file maps).
    let endpoint_index_id = crate::bootstrap::endpoint_index_id_for_storage(storage)?;
    let include_dedupe = matches!(config.remote_dedupe, RemoteDedupeMode::Disabled);
    let (exported_endpoint_db, export_stats) = export_endpoint_index_db_for_upload(
        &config.endpoint_db_path,
        &endpoint_temp_parent,
        include_dedupe,
    )
    .await?;
    debug!(
        event = "endpoint_index.export.finish",
        db_path = %config.endpoint_db_path.display(),
        source_bytes = export_stats.source_db_bytes,
        export_bytes = export_stats.export_db_bytes,
        "endpoint_index.export.finish"
    );
    let uploaded_endpoint = upload_index_sqlite_db(
        storage,
        &config,
        &endpoint_index_id,
        exported_endpoint_db.path(),
        &endpoint_temp_parent,
        &adaptive_controller,
        &upload_cancel,
        &rate_limiter,
        uploaded_bytes.as_ref(),
        uploaded_net_bytes.as_ref(),
        have_uploaded_net_bytes.as_ref(),
        upload_workload_total.as_ref(),
        upload_confirmed_bytes.as_ref(),
        options.progress,
        source_files_total,
        source_bytes_total,
        files_done_for_progress,
        scan_chunks_total.load(Ordering::Relaxed),
        scan_bytes_read.load(Ordering::Relaxed),
        uploaded_source_bytes.load(Ordering::Relaxed),
        scan_source_bytes_need_upload.load(Ordering::Relaxed),
        scan_bytes_deduped.load(Ordering::Relaxed),
    )
    .await?;

    execute_sqlite_with_busy_retry!(
        "endpoint_state.endpoint_index_id.upsert",
        sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
            .bind(crate::index_sync::ENDPOINT_STATE_ENDPOINT_INDEX_ID_KEY)
            .bind(&endpoint_index_id)
            .execute(&mut *conn)
    )?;
    execute_sqlite_with_busy_retry!(
        "endpoint_state.endpoint_manifest_object_id.upsert",
        sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
            .bind(crate::index_sync::ENDPOINT_STATE_ENDPOINT_MANIFEST_OBJECT_ID_KEY)
            .bind(&uploaded_endpoint.manifest_object_id)
            .execute(&mut *conn)
    )?;

    let mut index_parts_total = uploaded_filemap.manifest.parts.len() as u64
        + uploaded_endpoint.manifest.parts.len() as u64;
    if dedupe_enabled {
        let dedupe_conn = dedupe_conn.as_mut().ok_or_else(|| Error::InvalidConfig {
            message: "dedupe_db_path is required when remote_dedupe is enabled".to_string(),
        })?;
        let published = publish_remote_dedupe_if_needed(
            storage,
            &config,
            dedupe_conn,
            &adaptive_controller,
            &upload_cancel,
            &rate_limiter,
            uploaded_bytes.as_ref(),
            uploaded_net_bytes.as_ref(),
            have_uploaded_net_bytes.as_ref(),
            upload_workload_total.as_ref(),
            upload_confirmed_bytes.as_ref(),
            options.progress,
            source_files_total,
            source_bytes_total,
            files_done_for_progress,
            scan_chunks_total.load(Ordering::Relaxed),
            scan_bytes_read.load(Ordering::Relaxed),
            uploaded_source_bytes.load(Ordering::Relaxed),
            scan_source_bytes_need_upload.load(Ordering::Relaxed),
            scan_bytes_deduped.load(Ordering::Relaxed),
        )
        .await?;
        index_parts_total = index_parts_total.saturating_add(published.parts_uploaded);
    }

    result.index_parts = index_parts_total;
    result.bytes_uploaded = uploaded_bytes.load(Ordering::Relaxed);

    debug!(
        event = "phase.finish",
        phase = "index",
        duration_ms = index_started.elapsed().as_millis() as u64,
        index_parts = result.index_parts,
        "phase.finish"
    );

    // Post-success local maintenance: rewrite the endpoint index DB so only the latest snapshot
    // file maps for each source_path are kept. This keeps restore metadata (snapshots +
    // remote_indexes) while preventing multi-snapshot `files/file_chunks` growth from turning the
    // local DB into multi-GB uploads.
    //
    // Best-effort: backup correctness depends on remote uploads, not local compaction.
    drop(conn);
    let include_dedupe = matches!(config.remote_dedupe, RemoteDedupeMode::Disabled);
    if let Err(e) = compact_local_index_db(&config.endpoint_db_path, include_dedupe).await {
        warn!(
            event = "index.local_compact.failed",
            db_path = %config.endpoint_db_path.display(),
            error = %e,
            "index.local_compact.failed"
        );
    }

    Ok(result)
}

async fn schedule_pack_or_direct_upload(
    uploader: &UploadQueue,
    master_key: &[u8; 32],
    pack_state: &mut PackState,
    blob: SourceBlob,
) -> Result<()> {
    let SourceBlob {
        chunk_hash,
        blob,
        source_bytes,
    } = blob;

    if blob.len() + SINGLE_BLOB_PACK_OVERHEAD_BUDGET_BYTES > PACK_MAX_BYTES {
        flush_packer(uploader, master_key, pack_state).await?;
        uploader
            .enqueue_direct(chunk_hash, blob, source_bytes)
            .await?;
        return Ok(());
    }

    if !pack_state.packer.is_empty() && pack_state.packer.blob_len() + blob.len() > PACK_MAX_BYTES {
        flush_packer(uploader, master_key, pack_state).await?;
    }

    pack_state
        .staged_source_bytes
        .insert(chunk_hash.clone(), source_bytes);
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
            .map(|entry| {
                let chunk_hash = entry.chunk_hash;
                let source_bytes = pack_state
                    .staged_source_bytes
                    .remove(&chunk_hash)
                    .unwrap_or(entry.len);
                PackEntryRef {
                    chunk_hash,
                    offset: entry.offset,
                    len: entry.len,
                    source_bytes,
                }
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
    staged_source_bytes: HashMap<String, u64>,
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
            staged_source_bytes: HashMap::new(),
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

#[cfg(test)]
async fn insert_filemap_chunks_batch(
    conn: &mut DbConn,
    rows: &[FilemapChunkRow],
) -> Result<Vec<ScanSqliteRetryWait>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut retry_idx = 0usize;
    let mut retry_waits = Vec::new();
    'retry: loop {
        let mut tx = conn.begin().await.map_err(Error::from)?;

        for statement_rows in rows.chunks(FILEMAP_CHUNK_INSERT_ROWS_PER_STATEMENT) {
            let mut query = QueryBuilder::<Sqlite>::new(
                "INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) ",
            );
            query.push_values(statement_rows, |mut values, row| {
                values
                    .push_bind(&row.chunk_hash)
                    .push_bind(row.size)
                    .push_bind("blake3")
                    .push_bind("xchacha20poly1305")
                    .push("strftime('%Y-%m-%dT%H:%M:%fZ','now')");
            });
            if let Err(error) = query.build().execute(&mut *tx).await {
                let _ = tx.rollback().await;
                if wait_for_scan_sqlite_busy_retry(
                    "chunks.insert.filemap.batch",
                    &mut retry_idx,
                    &error,
                    &mut retry_waits,
                )
                .await
                {
                    continue 'retry;
                }
                return Err(Error::from(error));
            }
        }

        if let Err(error) = tx.commit().await {
            if wait_for_scan_sqlite_busy_retry(
                "chunks.insert.filemap.batch",
                &mut retry_idx,
                &error,
                &mut retry_waits,
            )
            .await
            {
                continue 'retry;
            }
            return Err(Error::from(error));
        }
        return Ok(retry_waits);
    }
}

#[cfg(test)]
async fn delete_transient_scan_file(
    conn: &mut DbConn,
    file_id: &str,
) -> Result<Vec<ScanSqliteRetryWait>> {
    let mut retry_idx = 0usize;
    let mut retry_waits = Vec::new();
    'retry: loop {
        let mut tx = conn.begin().await.map_err(Error::from)?;
        for statement in [
            "DELETE FROM file_chunks WHERE file_id = ?",
            "DELETE FROM files WHERE file_id = ?",
        ] {
            if let Err(error) = sqlx::query(statement).bind(file_id).execute(&mut *tx).await {
                let _ = tx.rollback().await;
                if wait_for_scan_sqlite_busy_retry(
                    "files.transient_delete",
                    &mut retry_idx,
                    &error,
                    &mut retry_waits,
                )
                .await
                {
                    continue 'retry;
                }
                return Err(Error::from(error));
            }
        }

        if let Err(error) = tx.commit().await {
            if wait_for_scan_sqlite_busy_retry(
                "files.transient_delete",
                &mut retry_idx,
                &error,
                &mut retry_waits,
            )
            .await
            {
                continue 'retry;
            }
            return Err(Error::from(error));
        }
        return Ok(retry_waits);
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

async fn dedupe_db_has_any_chunk_objects(conn: &mut DbConn) -> Result<bool> {
    let row = sqlx::query("SELECT 1 AS n FROM chunk_objects LIMIT 1")
        .fetch_optional(&mut **conn)
        .await?;
    Ok(row.is_some())
}

async fn seed_dedupe_db_from_endpoint_db(
    dedupe_conn: &mut DbConn,
    endpoint_db_path: &Path,
) -> Result<()> {
    if !endpoint_db_path.exists() {
        return Ok(());
    }

    // ATTACH needs a literal string; escape single quotes defensively.
    let src_path_sql = endpoint_db_path.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("ATTACH DATABASE '{src_path_sql}' AS src"))
        .execute(&mut **dedupe_conn)
        .await?;

    let mut tx = dedupe_conn.begin().await?;

    sqlx::query(
        r#"
        INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
        SELECT chunk_hash, size, hash_alg, enc_alg, created_at
        FROM src.chunks
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT OR REPLACE INTO chunk_objects (chunk_hash, provider, object_id, created_at)
        SELECT chunk_hash, provider, object_id, created_at
        FROM src.chunk_objects
        "#,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    sqlx::query("DETACH DATABASE src")
        .execute(&mut **dedupe_conn)
        .await?;

    Ok(())
}

async fn record_dedupe_chunk_objects_batch(
    dedupe_conn: &mut DbConn,
    pending_conn: &mut DbConn,
    provider: &str,
    chunk_objects: &[ChunkObjectMapping],
) -> Result<()> {
    record_dedupe_chunk_objects_batch_inner(dedupe_conn, provider, chunk_objects).await?;
    record_dedupe_chunk_objects_batch_inner(pending_conn, provider, chunk_objects).await?;
    Ok(())
}

async fn record_dedupe_chunk_objects_batch_inner(
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
                    operation = "dedupe.chunk_objects.begin_tx",
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
                INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
                VALUES (?, ?, 'blake3', 'xchacha20poly1305', strftime('%Y-%m-%dT%H:%M:%fZ','now'))
                "#,
            )
            .bind(&m.chunk_hash)
            .bind(m.source_bytes as i64)
            .execute(&mut *tx)
            .await
            {
                retry_err = Some(e);
                break;
            }

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
                    operation = "dedupe.chunk_objects.upsert_batch",
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
) -> Result<Vec<String>> {
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
    .bind(&source)
    .bind(keep_last_snapshots as i64)
    .fetch_all(&mut **conn)
    .await?;

    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let snapshot_ids = rows
        .into_iter()
        .map(|row| row.get::<String, _>("snapshot_id"))
        .collect::<Vec<_>>();

    let total_batches = snapshot_ids.len().div_ceil(RETENTION_SNAPSHOT_BATCH_SIZE);
    for (batch_idx, batch) in snapshot_ids
        .chunks(RETENTION_SNAPSHOT_BATCH_SIZE)
        .enumerate()
    {
        let batch_no = batch_idx + 1;
        let batch_ids = batch.to_vec();
        apply_retention_snapshot_batch(conn, &source, &batch_ids, batch_no, total_batches).await?;
    }

    Ok(snapshot_ids)
}

fn cleanup_filemap_cache_best_effort(filemap_dir: &Path, snapshot_ids: &[String]) {
    if snapshot_ids.is_empty() {
        return;
    }

    for snapshot_id in snapshot_ids {
        let path = filemap_dir.join(format!("{snapshot_id}.sqlite"));
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                warn!(
                    event = "filemap_cache.delete_failed",
                    snapshot_id,
                    path = %path.display(),
                    error = %e,
                    "filemap_cache.delete_failed"
                );
            }
        }
    }
}

async fn apply_retention_snapshot_batch(
    conn: &mut DbConn,
    source_path: &str,
    snapshot_ids: &[String],
    batch_no: usize,
    total_batches: usize,
) -> Result<()> {
    let mut retry_idx = 0usize;
    loop {
        let started = Instant::now();
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
        let mut deleted_file_rows = 0u64;
        let mut deleted_chunk_rows = 0u64;
        let mut file_batches = 0usize;

        match delete_files_and_chunks_for_snapshots(&mut tx, snapshot_ids).await {
            Ok(stats) => {
                deleted_file_rows = stats.deleted_files;
                deleted_chunk_rows = stats.deleted_chunks;
                file_batches = stats.file_batches;
            }
            Err(e) => retry_err = Some(e),
        }

        if retry_err.is_none()
            && let Err(e) = delete_rows_for_snapshot_ids(
                &mut tx,
                "DELETE FROM remote_index_parts WHERE snapshot_id IN (",
                snapshot_ids,
            )
            .await
        {
            retry_err = Some(e);
        }

        if retry_err.is_none()
            && let Err(e) = delete_rows_for_snapshot_ids(
                &mut tx,
                "DELETE FROM remote_indexes WHERE snapshot_id IN (",
                snapshot_ids,
            )
            .await
        {
            retry_err = Some(e);
        }

        if retry_err.is_none()
            && let Err(e) = delete_rows_for_snapshot_ids(
                &mut tx,
                "DELETE FROM tasks WHERE snapshot_id IN (",
                snapshot_ids,
            )
            .await
        {
            retry_err = Some(e);
        }

        if retry_err.is_none()
            && let Err(e) = clear_base_snapshot_refs_for_retention(&mut tx, snapshot_ids).await
        {
            retry_err = Some(e);
        }

        if retry_err.is_none()
            && let Err(e) = delete_rows_for_snapshot_ids(
                &mut tx,
                "DELETE FROM snapshots WHERE snapshot_id IN (",
                snapshot_ids,
            )
            .await
        {
            retry_err = Some(e);
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
                    source_path,
                    batch = batch_no,
                    batch_total = total_batches,
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

        debug!(
            event = "snapshots.retention.batch_done",
            source_path,
            batch = batch_no,
            batch_total = total_batches,
            snapshots = snapshot_ids.len(),
            file_batches,
            deleted_file_rows,
            deleted_chunk_rows,
            duration_ms = started.elapsed().as_millis() as u64,
            "snapshots.retention.batch_done"
        );
        return Ok(());
    }
}

#[derive(Default)]
struct RetentionDeleteStats {
    deleted_files: u64,
    deleted_chunks: u64,
    file_batches: usize,
}

async fn delete_files_and_chunks_for_snapshots(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    snapshot_ids: &[String],
) -> std::result::Result<RetentionDeleteStats, sqlx::Error> {
    let mut stats = RetentionDeleteStats::default();

    loop {
        let file_ids =
            select_file_ids_for_snapshots(tx, snapshot_ids, RETENTION_FILE_BATCH_SIZE).await?;
        if file_ids.is_empty() {
            break;
        }
        stats.file_batches += 1;
        stats.deleted_chunks +=
            delete_rows_for_file_ids(tx, "DELETE FROM file_chunks WHERE file_id IN (", &file_ids)
                .await?;
        stats.deleted_files +=
            delete_rows_for_file_ids(tx, "DELETE FROM files WHERE file_id IN (", &file_ids).await?;
    }

    Ok(stats)
}

async fn select_file_ids_for_snapshots(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    snapshot_ids: &[String],
    limit: usize,
) -> std::result::Result<Vec<String>, sqlx::Error> {
    let mut query = QueryBuilder::<Sqlite>::new("SELECT file_id FROM files WHERE snapshot_id IN (");
    push_string_bind_list(&mut query, snapshot_ids);
    query.push(") LIMIT ").push_bind(limit as i64);
    let rows = query.build().fetch_all(&mut **tx).await?;
    Ok(rows
        .into_iter()
        .map(|row| row.get::<String, _>("file_id"))
        .collect())
}

async fn delete_rows_for_file_ids(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    sql_prefix: &str,
    file_ids: &[String],
) -> std::result::Result<u64, sqlx::Error> {
    let mut query = QueryBuilder::<Sqlite>::new(sql_prefix);
    push_string_bind_list(&mut query, file_ids);
    query.push(")");
    Ok(query.build().execute(&mut **tx).await?.rows_affected())
}

async fn delete_rows_for_snapshot_ids(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    sql_prefix: &str,
    snapshot_ids: &[String],
) -> std::result::Result<u64, sqlx::Error> {
    let mut query = QueryBuilder::<Sqlite>::new(sql_prefix);
    push_string_bind_list(&mut query, snapshot_ids);
    query.push(")");
    Ok(query.build().execute(&mut **tx).await?.rows_affected())
}

async fn clear_base_snapshot_refs_for_retention(
    tx: &mut sqlx::Transaction<'_, Sqlite>,
    snapshot_ids: &[String],
) -> std::result::Result<u64, sqlx::Error> {
    let mut query = QueryBuilder::<Sqlite>::new(
        "UPDATE snapshots SET base_snapshot_id = NULL WHERE base_snapshot_id IN (",
    );
    push_string_bind_list(&mut query, snapshot_ids);
    query.push(")");
    Ok(query.build().execute(&mut **tx).await?.rows_affected())
}

fn push_string_bind_list<'a>(query: &mut QueryBuilder<'a, Sqlite>, values: &'a [String]) {
    let mut separated = query.separated(", ");
    for value in values {
        separated.push_bind(value);
    }
}

async fn compact_index_db_if_needed(conn: &mut DbConn, db_path: &Path) {
    let page_count = match sqlx::query("PRAGMA page_count")
        .fetch_one(&mut **conn)
        .await
    {
        Ok(row) => row.try_get::<i64, _>(0).unwrap_or(0),
        Err(e) => {
            warn!(
                event = "index.compact.page_count_failed",
                db_path = %db_path.display(),
                error = %e,
                "index.compact.page_count_failed"
            );
            return;
        }
    };
    let free_pages = match sqlx::query("PRAGMA freelist_count")
        .fetch_one(&mut **conn)
        .await
    {
        Ok(row) => row.try_get::<i64, _>(0).unwrap_or(0),
        Err(e) => {
            warn!(
                event = "index.compact.freelist_failed",
                db_path = %db_path.display(),
                error = %e,
                "index.compact.freelist_failed"
            );
            return;
        }
    };

    if page_count <= 0 || free_pages <= 0 {
        return;
    }

    let free_ratio = free_pages as f64 / page_count as f64;
    if page_count < INDEX_COMPACT_MIN_PAGE_COUNT
        || free_pages < INDEX_COMPACT_MIN_FREE_PAGES
        || free_ratio < INDEX_COMPACT_MIN_FREE_RATIO
    {
        return;
    }

    debug!(
        event = "index.compact.start",
        db_path = %db_path.display(),
        page_count,
        free_pages,
        free_ratio,
        "index.compact.start"
    );

    let _ = sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .fetch_all(&mut **conn)
        .await;
    let started = Instant::now();
    match execute_sqlite_with_busy_retry!(
        "index.compact.vacuum",
        sqlx::query("VACUUM").execute(&mut **conn)
    ) {
        Ok(_) => {
            debug!(
                event = "index.compact.finish",
                db_path = %db_path.display(),
                duration_ms = started.elapsed().as_millis() as u64,
                "index.compact.finish"
            );
        }
        Err(e) => {
            warn!(
                event = "index.compact.failed",
                db_path = %db_path.display(),
                error = %e,
                "index.compact.failed"
            );
        }
    }
}

async fn latest_snapshot_for_source(
    conn: &mut DbConn,
    source_path: &Path,
    provider: &str,
) -> Result<Option<String>> {
    let source = path_to_utf8(source_path)?;
    let kind = provider_kind(provider);
    let like = format!("{kind}%");
    let row = sqlx::query(
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
    .bind(source)
    .bind(provider)
    .bind(like)
    .fetch_optional(&mut **conn)
    .await?;

    Ok(row.map(|r| r.get::<String, _>("snapshot_id")))
}

fn provider_kind(provider: &str) -> &str {
    provider.split(['/', ':']).next().unwrap_or(provider).trim()
}

async fn attach_db(conn: &mut DbConn, alias: &str, path: &Path) -> Result<()> {
    // ATTACH needs a literal string; escape single quotes defensively.
    let path_sql = path.to_string_lossy().replace('\'', "''");
    let sql = format!("ATTACH DATABASE '{path_sql}' AS {alias}");
    sqlx::query(&sql).execute(&mut **conn).await?;
    Ok(())
}

async fn endpoint_db_has_snapshot_filemap(conn: &mut DbConn, snapshot_id: &str) -> Result<bool> {
    let row = sqlx::query("SELECT 1 as present FROM files WHERE snapshot_id = ? LIMIT 1")
        .bind(snapshot_id)
        .fetch_optional(&mut **conn)
        .await?;
    Ok(row.is_some())
}

async fn lookup_remote_index_manifest_object_id(
    conn: &mut DbConn,
    snapshot_id: &str,
    provider: &str,
) -> Result<Option<String>> {
    let row = sqlx::query(
        "SELECT provider, manifest_object_id FROM remote_indexes WHERE snapshot_id = ? LIMIT 1",
    )
    .bind(snapshot_id)
    .fetch_optional(&mut **conn)
    .await?;

    let Some(row) = row else {
        return Ok(None);
    };

    let row_provider: String = row.get("provider");
    let manifest_object_id: String = row.get("manifest_object_id");

    if row_provider == provider || provider_kind(&row_provider) == provider_kind(provider) {
        Ok(Some(manifest_object_id))
    } else {
        Ok(None)
    }
}

fn base_copy_rows_bytes(rows: &[BaseFileChunkCopyRow]) -> u64 {
    rows.iter()
        .fold(0u64, |acc, row| acc.saturating_add(row.size))
}

#[cfg(test)]
async fn initialize_base_chunk_copy_map(conn: &mut DbConn) -> Result<Vec<ScanSqliteRetryWait>> {
    let mut retry_idx = 0usize;
    let mut retry_waits = Vec::new();
    loop {
        match sqlx::query(
            "CREATE TEMP TABLE IF NOT EXISTS base_copy_map (\
                file_id TEXT PRIMARY KEY,\
                base_file_id TEXT NOT NULL\
            )",
        )
        .execute(&mut **conn)
        .await
        {
            Ok(_) => return Ok(retry_waits),
            Err(error) => {
                if wait_for_scan_sqlite_busy_retry(
                    "base_copy.map_init",
                    &mut retry_idx,
                    &error,
                    &mut retry_waits,
                )
                .await
                {
                    continue;
                }
                return Err(Error::from(error));
            }
        }
    }
}

#[cfg(test)]
async fn stage_base_chunk_copy_batch(
    conn: &mut DbConn,
    rows: &mut Vec<BaseFileChunkCopyRow>,
) -> Result<Vec<ScanSqliteRetryWait>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut retry_idx = 0usize;
    let mut retry_waits = Vec::new();
    'retry: loop {
        let mut tx = conn.begin().await.map_err(Error::from)?;
        let mut map_query =
            QueryBuilder::<Sqlite>::new("INSERT INTO temp.base_copy_map (file_id, base_file_id) ");
        map_query.push_values(rows.iter(), |mut values, row| {
            values.push_bind(&row.file_id).push_bind(&row.base_file_id);
        });
        match map_query.build().execute(&mut *tx).await {
            Ok(v) => v.rows_affected(),
            Err(e) => {
                let _ = tx.rollback().await;
                if wait_for_scan_sqlite_busy_retry(
                    "base_copy.map_stage",
                    &mut retry_idx,
                    &e,
                    &mut retry_waits,
                )
                .await
                {
                    continue 'retry;
                }
                return Err(Error::from(e));
            }
        };

        if let Err(e) = tx.commit().await {
            if wait_for_scan_sqlite_busy_retry(
                "base_copy.map_stage",
                &mut retry_idx,
                &e,
                &mut retry_waits,
            )
            .await
            {
                continue 'retry;
            }
            return Err(Error::from(e));
        }
        rows.clear();
        return Ok(retry_waits);
    }
}

#[cfg(test)]
async fn materialize_base_chunk_copy_map(
    conn: &mut DbConn,
) -> Result<(u64, Vec<ScanSqliteRetryWait>)> {
    let mut retry_idx = 0usize;
    let mut retry_waits = Vec::new();
    'retry: loop {
        let mut tx = conn.begin().await.map_err(Error::from)?;
        // The referenced chunk rows are seeded once per base snapshot. Keeping all
        // mappings in the temp table lets SQLite materialize the file-chunk rows in
        // one transaction instead of reopening and committing this join per batch.
        let result = sqlx::query(
            r#"
            INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len)
            SELECT m.file_id, fc.seq, fc.chunk_hash, fc.offset, fc.len
            FROM temp.base_copy_map m
            JOIN base.file_chunks fc ON fc.file_id = m.base_file_id
            "#,
        )
        .execute(&mut *tx)
        .await;
        let copied_chunks = match result {
            Ok(value) => value.rows_affected(),
            Err(error) => {
                let _ = tx.rollback().await;
                if wait_for_scan_sqlite_busy_retry(
                    "base_copy.materialize",
                    &mut retry_idx,
                    &error,
                    &mut retry_waits,
                )
                .await
                {
                    continue 'retry;
                }
                return Err(Error::from(error));
            }
        };
        if let Err(error) = tx.commit().await {
            if wait_for_scan_sqlite_busy_retry(
                "base_copy.materialize",
                &mut retry_idx,
                &error,
                &mut retry_waits,
            )
            .await
            {
                continue 'retry;
            }
            return Err(Error::from(error));
        }
        return Ok((copied_chunks, retry_waits));
    }
}

#[cfg(test)]
async fn seed_base_snapshot_chunks(
    conn: &mut DbConn,
    base_snapshot_id: &str,
) -> Result<Vec<ScanSqliteRetryWait>> {
    let mut retry_idx = 0usize;
    let mut retry_waits = Vec::new();
    loop {
        let mut tx = conn.begin().await.map_err(Error::from)?;
        let result = sqlx::query(
            r#"
            INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
            SELECT c.chunk_hash, c.size, c.hash_alg, c.enc_alg, c.created_at
            FROM base.files f
            JOIN base.file_chunks fc ON fc.file_id = f.file_id
            JOIN base.chunks c ON c.chunk_hash = fc.chunk_hash
            WHERE f.snapshot_id = ? AND f.kind = 'file'
            "#,
        )
        .bind(base_snapshot_id)
        .execute(&mut *tx)
        .await;

        match result {
            Ok(_) => {
                if let Err(error) = tx.commit().await {
                    if wait_for_scan_sqlite_busy_retry(
                        "chunks.copy_from_base.snapshot",
                        &mut retry_idx,
                        &error,
                        &mut retry_waits,
                    )
                    .await
                    {
                        continue;
                    }
                    return Err(Error::from(error));
                }
                return Ok(retry_waits);
            }
            Err(error) => {
                let _ = tx.rollback().await;
                if wait_for_scan_sqlite_busy_retry(
                    "chunks.copy_from_base.snapshot",
                    &mut retry_idx,
                    &error,
                    &mut retry_waits,
                )
                .await
                {
                    continue;
                }
                return Err(Error::from(error));
            }
        }
    }
}

async fn insert_file_chunks_batch_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    file_id: &str,
    rows: &[FileChunkRow],
) -> Result<Vec<ScanSqliteRetryWait>> {
    for statement_rows in rows.chunks(FILE_CHUNK_INSERT_ROWS_PER_STATEMENT) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len) ",
        );
        query.push_values(statement_rows, |mut values, row| {
            values
                .push_bind(file_id)
                .push_bind(row.seq)
                .push_bind(&row.chunk_hash)
                .push_bind(row.offset)
                .push_bind(row.len);
        });
        query.build().execute(&mut **tx).await?;
    }
    Ok(Vec::new())
}

async fn insert_filemap_chunks_batch_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &[FilemapChunkRow],
) -> Result<Vec<ScanSqliteRetryWait>> {
    for statement_rows in rows.chunks(FILEMAP_CHUNK_INSERT_ROWS_PER_STATEMENT) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) ",
        );
        query.push_values(statement_rows, |mut values, row| {
            values
                .push_bind(&row.chunk_hash)
                .push_bind(row.size)
                .push_bind("blake3")
                .push_bind("xchacha20poly1305")
                .push("strftime('%Y-%m-%dT%H:%M:%fZ','now')");
        });
        query.build().execute(&mut **tx).await?;
    }
    Ok(Vec::new())
}

async fn insert_scan_file_rows_batch_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    snapshot_id: &str,
    rows: &[PendingScanEntry],
) -> Result<Vec<ScanSqliteRetryWait>> {
    for statement_rows in rows.chunks(SCAN_FILE_INSERT_ROWS_PER_STATEMENT) {
        let mut query = QueryBuilder::<Sqlite>::new(
            "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) ",
        );
        query.push_values(statement_rows, |mut values, row| {
            values
                .push_bind(&row.file_id)
                .push_bind(snapshot_id)
                .push_bind(&row.rel_path)
                .push_bind(row.size)
                .push_bind(row.mtime_ms)
                .push_bind(row.mode)
                .push_bind(row.kind);
        });
        query.build().execute(&mut **tx).await?;
    }
    Ok(Vec::new())
}

async fn update_scan_file_metadata_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    file_id: &str,
    size: i64,
    mtime_ms: i64,
    mode: i64,
) -> Result<Vec<ScanSqliteRetryWait>> {
    sqlx::query("UPDATE files SET size = ?, mtime_ms = ?, mode = ? WHERE file_id = ?")
        .bind(size)
        .bind(mtime_ms)
        .bind(mode)
        .bind(file_id)
        .execute(&mut **tx)
        .await?;
    Ok(Vec::new())
}

async fn delete_transient_scan_file_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    file_id: &str,
) -> Result<Vec<ScanSqliteRetryWait>> {
    sqlx::query("DELETE FROM file_chunks WHERE file_id = ?")
        .bind(file_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM files WHERE file_id = ?")
        .bind(file_id)
        .execute(&mut **tx)
        .await?;
    Ok(Vec::new())
}

async fn lookup_base_file_snapshot_rows_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    base_snapshot_id: &str,
    entries: &[PendingScanEntry],
) -> Result<(
    HashMap<String, BaseFileSnapshotRow>,
    Vec<ScanSqliteRetryWait>,
)> {
    let file_paths: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.kind == "file")
        .map(|entry| entry.rel_path.as_str())
        .collect();
    if file_paths.is_empty() {
        return Ok((HashMap::new(), Vec::new()));
    }

    let mut query = QueryBuilder::<Sqlite>::new("WITH requested(path) AS (");
    query.push_values(file_paths.iter(), |mut values, path| {
        values.push_bind(*path);
    });
    query.push(
        ") SELECT f.file_id, f.path, f.size, f.mtime_ms, f.mode \
         FROM requested r \
         CROSS JOIN base.files f WHERE f.snapshot_id = ",
    );
    query.push_bind(base_snapshot_id);
    query.push(" AND f.path = r.path AND f.kind = 'file'");

    let rows = query.build().fetch_all(&mut **tx).await?;
    Ok((
        rows.into_iter()
            .map(|row| {
                (
                    row.get::<String, _>("path"),
                    BaseFileSnapshotRow {
                        file_id: row.get::<String, _>("file_id"),
                        size: row.get::<i64, _>("size"),
                        mtime_ms: row.get::<i64, _>("mtime_ms"),
                        mode: row.get::<i64, _>("mode"),
                    },
                )
            })
            .collect(),
        Vec::new(),
    ))
}

async fn initialize_base_chunk_copy_map_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> Result<Vec<ScanSqliteRetryWait>> {
    sqlx::query(
        "CREATE TEMP TABLE IF NOT EXISTS base_copy_map (\
            file_id TEXT PRIMARY KEY,\
            base_file_id TEXT NOT NULL\
        )",
    )
    .execute(&mut **tx)
    .await?;
    Ok(Vec::new())
}

async fn stage_base_chunk_copy_batch_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    rows: &mut Vec<BaseFileChunkCopyRow>,
) -> Result<Vec<ScanSqliteRetryWait>> {
    if rows.is_empty() {
        return Ok(Vec::new());
    }

    let mut query =
        QueryBuilder::<Sqlite>::new("INSERT INTO temp.base_copy_map (file_id, base_file_id) ");
    query.push_values(rows.iter(), |mut values, row| {
        values.push_bind(&row.file_id).push_bind(&row.base_file_id);
    });
    query.build().execute(&mut **tx).await?;
    rows.clear();
    Ok(Vec::new())
}

async fn materialize_base_chunk_copy_map_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
) -> Result<(u64, Vec<ScanSqliteRetryWait>)> {
    let copied_chunks = sqlx::query(
        r#"
        INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len)
        SELECT m.file_id, fc.seq, fc.chunk_hash, fc.offset, fc.len
        FROM temp.base_copy_map m
        JOIN base.file_chunks fc ON fc.file_id = m.base_file_id
        "#,
    )
    .execute(&mut **tx)
    .await?
    .rows_affected();
    Ok((copied_chunks, Vec::new()))
}

async fn seed_base_snapshot_chunks_in_tx(
    tx: &mut Transaction<'_, Sqlite>,
    base_snapshot_id: &str,
) -> Result<Vec<ScanSqliteRetryWait>> {
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
        SELECT c.chunk_hash, c.size, c.hash_alg, c.enc_alg, c.created_at
        FROM base.files f
        JOIN base.file_chunks fc ON fc.file_id = f.file_id
        JOIN base.chunks c ON c.chunk_hash = fc.chunk_hash
        WHERE f.snapshot_id = ? AND f.kind = 'file'
        "#,
    )
    .bind(base_snapshot_id)
    .execute(&mut **tx)
    .await?;
    Ok(Vec::new())
}

async fn checkpoint_snapshot_filemap_wal(conn: &mut DbConn) -> Result<(i64, i64)> {
    // Scan-time sync is intentionally disabled for this temporary DB. Restore
    // durable sync before folding the WAL into the uploaded main database.
    sqlx::query("PRAGMA synchronous = FULL")
        .execute(&mut **conn)
        .await?;
    let row = sqlx::query("PRAGMA main.wal_checkpoint(TRUNCATE)")
        .fetch_one(&mut **conn)
        .await?;
    let busy = row.get::<i64, _>(0);
    let wal_log_frames = row.get::<i64, _>(1);
    let wal_checkpointed_frames = row.get::<i64, _>(2);
    if busy != 0 {
        return Err(Error::Integrity {
            message: "snapshot filemap WAL checkpoint remained busy".to_string(),
        });
    }
    Ok((wal_log_frames, wal_checkpointed_frames))
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
    let kind = provider_kind(provider);
    let like = format!("{kind}%");
    let rows: Vec<sqlx::sqlite::SqliteRow> = sqlx::query(
        r#"
        SELECT chunk_hash, object_id
        FROM chunk_objects
        WHERE provider = ? OR provider LIKE ?
        "#,
    )
    .bind(provider)
    .bind(&like)
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

#[derive(Debug, Clone, Copy)]
struct EndpointIndexExportStats {
    source_db_bytes: u64,
    export_db_bytes: u64,
}

async fn export_endpoint_index_db_for_upload(
    source_db_path: &Path,
    temp_parent: &Path,
    include_dedupe: bool,
) -> Result<(tempfile::NamedTempFile, EndpointIndexExportStats)> {
    let source_db_bytes = source_db_path.metadata()?.len();

    let exported_db = tempfile::Builder::new()
        .prefix("televy-index-export-")
        .suffix(".sqlite")
        .tempfile_in(temp_parent)?;
    let export_path = exported_db.path().to_path_buf();

    let pool = open_index_db(&export_path).await?;
    let mut conn: DbConn = pool.acquire().await?;
    drop(pool);

    // ATTACH needs a literal string; escape single quotes defensively.
    let src_path_sql = source_db_path.to_string_lossy().replace('\'', "''");
    sqlx::query(&format!("ATTACH DATABASE '{src_path_sql}' AS src"))
        .execute(&mut *conn)
        .await?;

    // Copy in one transaction to keep export fast.
    let mut tx = conn.begin().await?;
    sqlx::query(
        r#"
        INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id)
        SELECT snapshot_id, created_at, source_path, label, base_snapshot_id
        FROM src.snapshots
        "#,
    )
    .execute(&mut *tx)
    .await?;

    if include_dedupe {
        sqlx::query(
            r#"
            INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
            SELECT chunk_hash, size, hash_alg, enc_alg, created_at
            FROM src.chunks
            "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at)
            SELECT chunk_hash, provider, object_id, created_at
            FROM src.chunk_objects
            "#,
        )
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query(
        r#"
        INSERT INTO remote_indexes (snapshot_id, provider, manifest_object_id, created_at)
        SELECT snapshot_id, provider, manifest_object_id, created_at
        FROM src.remote_indexes
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO remote_index_parts (snapshot_id, part_no, provider, object_id, size, hash)
        SELECT snapshot_id, part_no, provider, object_id, size, hash
        FROM src.remote_index_parts
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO endpoint_state (key, value)
        SELECT key, value
        FROM src.endpoint_state
        "#,
    )
    .execute(&mut *tx)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO tasks (
            task_id, kind, state, created_at, started_at, finished_at, snapshot_id, error_code, error_message
        )
        SELECT
            task_id, kind, state, created_at, started_at, finished_at, snapshot_id, error_code, error_message
        FROM src.tasks
        "#,
    )
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;

    sqlx::query("DETACH DATABASE src")
        .execute(&mut *conn)
        .await?;
    drop(conn);

    let export_db_bytes = export_path.metadata()?.len();
    Ok((
        exported_db,
        EndpointIndexExportStats {
            source_db_bytes,
            export_db_bytes,
        },
    ))
}

#[derive(Debug, Clone)]
struct PublishedDedupe {
    parts_uploaded: u64,
}

#[allow(clippy::too_many_arguments)]
async fn publish_remote_dedupe_if_needed<S: Storage>(
    storage: &S,
    config: &BackupConfig,
    dedupe_conn: &mut DbConn,
    adaptive_controller: &Arc<AdaptiveUploadController>,
    cancel: &CancellationToken,
    rate_limiter: &UploadRateLimiter,
    uploaded_bytes: &AtomicU64,
    uploaded_net_bytes: &AtomicU64,
    have_uploaded_net_bytes: &AtomicBool,
    upload_workload_total: &AtomicU64,
    upload_confirmed_bytes: &AtomicU64,
    progress: Option<&dyn ProgressSink>,
    source_files_total: Option<u64>,
    source_bytes_total: Option<u64>,
    files_indexed: u64,
    chunks_total: u64,
    bytes_read: u64,
    bytes_uploaded_source: u64,
    source_bytes_need_upload_total: u64,
    bytes_deduped: u64,
) -> Result<PublishedDedupe> {
    let dedupe_temp_parent = config
        .dedupe_db_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);

    match &config.remote_dedupe {
        RemoteDedupeMode::Disabled => Ok(PublishedDedupe { parts_uploaded: 0 }),
        RemoteDedupeMode::Enable { endpoint_dedupe_id } => {
            let base_id = dedupe_base_id_for_storage(storage);
            let exported_base =
                export_dedupe_db_for_upload(&config.dedupe_db_path, &dedupe_temp_parent).await?;
            let uploaded_base = upload_index_sqlite_db(
                storage,
                config,
                &base_id,
                exported_base.path(),
                &dedupe_temp_parent,
                adaptive_controller,
                cancel,
                rate_limiter,
                uploaded_bytes,
                uploaded_net_bytes,
                have_uploaded_net_bytes,
                upload_workload_total,
                upload_confirmed_bytes,
                progress,
                source_files_total,
                source_bytes_total,
                files_indexed,
                chunks_total,
                bytes_read,
                bytes_uploaded_source,
                source_bytes_need_upload_total,
                bytes_deduped,
            )
            .await?;

            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let cat = DedupeCatalogV1 {
                version: DEDUPE_CATALOG_VERSION,
                updated_at: now.clone(),
                endpoint_dedupe_id: endpoint_dedupe_id.clone(),
                base: DedupeCatalogBase {
                    base_id,
                    manifest_object_id: uploaded_base.manifest_object_id.clone(),
                },
                deltas: Vec::new(),
            };

            rate_limiter.wait_turn().await;
            let catalog_object_id =
                save_remote_dedupe_catalog(storage, &config.master_key, &cat).await?;

            // Only clear pending once base+catalog is fully published.
            reset_dedupe_pending_spool_db(&config.dedupe_pending_db_path).await?;

            // Record state so future runs can skip rebuilding when bootstrap points at this catalog.
            execute_sqlite_with_busy_retry!(
                "endpoint_state.endpoint_dedupe_id.upsert",
                sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
                    .bind(crate::index_sync::ENDPOINT_STATE_ENDPOINT_DEDUPE_ID_KEY)
                    .bind(endpoint_dedupe_id)
                    .execute(&mut **dedupe_conn)
            )?;
            execute_sqlite_with_busy_retry!(
                "endpoint_state.dedupe_catalog_object_id.upsert",
                sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
                    .bind(crate::index_sync::ENDPOINT_STATE_DEDUPE_CATALOG_OBJECT_ID_KEY)
                    .bind(&catalog_object_id)
                    .execute(&mut **dedupe_conn)
            )?;

            debug!(
                event = "dedupe.publish.enable.finish",
                endpoint_dedupe_id,
                catalog_object_id,
                base_manifest_object_id = %uploaded_base.manifest_object_id,
                base_parts = uploaded_base.manifest.parts.len() as u64,
                "dedupe.publish.enable.finish"
            );

            Ok(PublishedDedupe {
                parts_uploaded: uploaded_base.manifest.parts.len() as u64,
            })
        }
        RemoteDedupeMode::Incremental {
            endpoint_dedupe_id,
            catalog_object_id,
        } => {
            let mut parts_uploaded = 0u64;

            // Always ensure local state is populated (even if no deltas are generated).
            execute_sqlite_with_busy_retry!(
                "endpoint_state.endpoint_dedupe_id.upsert",
                sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
                    .bind(crate::index_sync::ENDPOINT_STATE_ENDPOINT_DEDUPE_ID_KEY)
                    .bind(endpoint_dedupe_id)
                    .execute(&mut **dedupe_conn)
            )?;
            execute_sqlite_with_busy_retry!(
                "endpoint_state.dedupe_catalog_object_id.upsert",
                sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
                    .bind(crate::index_sync::ENDPOINT_STATE_DEDUPE_CATALOG_OBJECT_ID_KEY)
                    .bind(catalog_object_id)
                    .execute(&mut **dedupe_conn)
            )?;

            if !db_path_has_any_chunk_objects(&config.dedupe_pending_db_path).await? {
                return Ok(PublishedDedupe { parts_uploaded });
            }

            let mut cat =
                load_remote_dedupe_catalog(storage, &config.master_key, catalog_object_id).await?;
            if cat.endpoint_dedupe_id != *endpoint_dedupe_id {
                return Err(Error::InvalidConfig {
                    message: format!(
                        "dedupe catalog endpoint mismatch: expected={} got={}",
                        endpoint_dedupe_id, cat.endpoint_dedupe_id
                    ),
                });
            }

            let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
            let next_catalog_object_id = if cat.deltas.len() >= DEDUPE_MAX_DELTAS_BEFORE_COMPACT {
                // Compaction: re-upload a fresh base DB and reset deltas.
                let base_id = cat.base.base_id.clone();
                let exported_base =
                    export_dedupe_db_for_upload(&config.dedupe_db_path, &dedupe_temp_parent)
                        .await?;
                let uploaded_base = upload_index_sqlite_db(
                    storage,
                    config,
                    &base_id,
                    exported_base.path(),
                    &dedupe_temp_parent,
                    adaptive_controller,
                    cancel,
                    rate_limiter,
                    uploaded_bytes,
                    uploaded_net_bytes,
                    have_uploaded_net_bytes,
                    upload_workload_total,
                    upload_confirmed_bytes,
                    progress,
                    source_files_total,
                    source_bytes_total,
                    files_indexed,
                    chunks_total,
                    bytes_read,
                    bytes_uploaded_source,
                    source_bytes_need_upload_total,
                    bytes_deduped,
                )
                .await?;
                parts_uploaded =
                    parts_uploaded.saturating_add(uploaded_base.manifest.parts.len() as u64);

                let new_cat = DedupeCatalogV1 {
                    version: DEDUPE_CATALOG_VERSION,
                    updated_at: now.clone(),
                    endpoint_dedupe_id: endpoint_dedupe_id.clone(),
                    base: DedupeCatalogBase {
                        base_id,
                        manifest_object_id: uploaded_base.manifest_object_id.clone(),
                    },
                    deltas: Vec::new(),
                };

                rate_limiter.wait_turn().await;
                let new_id =
                    save_remote_dedupe_catalog(storage, &config.master_key, &new_cat).await?;

                reset_dedupe_pending_spool_db(&config.dedupe_pending_db_path).await?;

                debug!(
                    event = "dedupe.publish.compaction.finish",
                    endpoint_dedupe_id,
                    old_catalog_object_id = catalog_object_id,
                    new_catalog_object_id = %new_id,
                    base_manifest_object_id = %uploaded_base.manifest_object_id,
                    base_parts = uploaded_base.manifest.parts.len() as u64,
                    "dedupe.publish.compaction.finish"
                );

                new_id
            } else {
                let scope = storage
                    .object_id_scope()
                    .unwrap_or_else(|| storage.provider());
                let uuid_simple = uuid::Uuid::new_v4().simple().to_string();
                let delta_id = dedupe_delta_id_from_scope(scope, &uuid_simple);

                let exported_delta = export_pending_dedupe_delta_db(
                    &config.dedupe_pending_db_path,
                    &dedupe_temp_parent,
                )
                .await?;
                let delta_bytes = exported_delta
                    .path()
                    .metadata()
                    .map(|m| m.len())
                    .unwrap_or(0);
                let uploaded_delta = upload_index_sqlite_db(
                    storage,
                    config,
                    &delta_id,
                    exported_delta.path(),
                    &dedupe_temp_parent,
                    adaptive_controller,
                    cancel,
                    rate_limiter,
                    uploaded_bytes,
                    uploaded_net_bytes,
                    have_uploaded_net_bytes,
                    upload_workload_total,
                    upload_confirmed_bytes,
                    progress,
                    source_files_total,
                    source_bytes_total,
                    files_indexed,
                    chunks_total,
                    bytes_read,
                    bytes_uploaded_source,
                    source_bytes_need_upload_total,
                    bytes_deduped,
                )
                .await?;
                parts_uploaded =
                    parts_uploaded.saturating_add(uploaded_delta.manifest.parts.len() as u64);

                cat.updated_at = now.clone();
                cat.deltas.push(DedupeCatalogDelta {
                    delta_id,
                    manifest_object_id: uploaded_delta.manifest_object_id.clone(),
                    created_at: now.clone(),
                    bytes: Some(delta_bytes),
                });

                rate_limiter.wait_turn().await;
                let new_id = save_remote_dedupe_catalog(storage, &config.master_key, &cat).await?;

                reset_dedupe_pending_spool_db(&config.dedupe_pending_db_path).await?;

                debug!(
                    event = "dedupe.publish.delta.finish",
                    endpoint_dedupe_id,
                    old_catalog_object_id = catalog_object_id,
                    new_catalog_object_id = %new_id,
                    delta_manifest_object_id = %uploaded_delta.manifest_object_id,
                    delta_parts = uploaded_delta.manifest.parts.len() as u64,
                    "dedupe.publish.delta.finish"
                );

                new_id
            };

            // Record the catalog pointer for next run/bootstrap update.
            execute_sqlite_with_busy_retry!(
                "endpoint_state.dedupe_catalog_object_id.upsert",
                sqlx::query("INSERT OR REPLACE INTO endpoint_state (key, value) VALUES (?, ?)")
                    .bind(crate::index_sync::ENDPOINT_STATE_DEDUPE_CATALOG_OBJECT_ID_KEY)
                    .bind(&next_catalog_object_id)
                    .execute(&mut **dedupe_conn)
            )?;

            Ok(PublishedDedupe { parts_uploaded })
        }
    }
}

async fn db_path_has_any_chunk_objects(db_path: &Path) -> Result<bool> {
    if !db_path.exists() {
        return Ok(false);
    }
    let pool = match open_existing_index_db(db_path).await {
        Ok(pool) => pool,
        Err(_) => return Ok(false),
    };
    let row = sqlx::query("SELECT 1 AS n FROM chunk_objects LIMIT 1")
        .fetch_optional(&pool)
        .await?;
    Ok(row.is_some())
}

async fn export_dedupe_db_for_upload(
    source_db_path: &Path,
    temp_parent: &Path,
) -> Result<tempfile::NamedTempFile> {
    export_dedupe_tables_only_db(source_db_path, temp_parent, "televy-dedupe-base-export-").await
}

async fn export_pending_dedupe_delta_db(
    pending_db_path: &Path,
    temp_parent: &Path,
) -> Result<tempfile::NamedTempFile> {
    export_dedupe_tables_only_db(pending_db_path, temp_parent, "televy-dedupe-delta-export-").await
}

async fn export_dedupe_tables_only_db(
    source_db_path: &Path,
    temp_parent: &Path,
    prefix: &str,
) -> Result<tempfile::NamedTempFile> {
    let exported_db = tempfile::Builder::new()
        .prefix(prefix)
        .suffix(".sqlite")
        .tempfile_in(temp_parent)?;
    let export_path = exported_db.path().to_path_buf();

    // Create schema.
    let pool = open_index_db(&export_path).await?;
    let mut conn: DbConn = pool.acquire().await?;
    drop(pool);

    if source_db_path.exists() {
        // ATTACH needs a literal string; escape single quotes defensively.
        let src_path_sql = source_db_path.to_string_lossy().replace('\'', "''");
        sqlx::query(&format!("ATTACH DATABASE '{src_path_sql}' AS src"))
            .execute(&mut *conn)
            .await?;

        let mut tx = conn.begin().await?;

        sqlx::query(
            r#"
            INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at)
            SELECT chunk_hash, size, hash_alg, enc_alg, created_at
            FROM src.chunks
            "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at)
            SELECT chunk_hash, provider, object_id, created_at
            FROM src.chunk_objects
            "#,
        )
        .execute(&mut *tx)
        .await?;

        tx.commit().await?;

        sqlx::query("DETACH DATABASE src")
            .execute(&mut *conn)
            .await?;
    }

    drop(conn);
    Ok(exported_db)
}

async fn reset_dedupe_pending_spool_db(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let temp_parent = path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let tmp = tempfile::Builder::new()
        .prefix("televy-dedupe-pending-reset-")
        .suffix(".sqlite")
        .tempfile_in(&temp_parent)?;
    let tmp_path = tmp.path().to_path_buf();
    // Ensure schema exists (migrations).
    let _ = open_index_db(&tmp_path).await?;

    let (file, kept_path) = tmp.keep().map_err(|e| Error::Io(e.error))?;
    drop(file);
    replace_atomic(&kept_path, path).map_err(Error::Io)?;
    Ok(())
}

async fn compact_local_index_db(db_path: &Path, include_dedupe: bool) -> Result<()> {
    let temp_parent = db_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(std::env::temp_dir);
    let (exported_db, export_stats) =
        export_endpoint_index_db_for_upload(db_path, &temp_parent, include_dedupe).await?;

    debug!(
        event = "index.local_compact.export.finish",
        db_path = %db_path.display(),
        source_bytes = export_stats.source_db_bytes,
        export_bytes = export_stats.export_db_bytes,
        "index.local_compact.export.finish"
    );

    let (file, tmp_path) = exported_db.keep().map_err(|e| Error::Io(e.error))?;
    drop(file);
    if let Err(e) = replace_atomic(&tmp_path, db_path) {
        let _ = std::fs::remove_file(&tmp_path);
        return Err(Error::Io(e));
    }

    let new_bytes = db_path.metadata().map(|m| m.len()).unwrap_or(0);
    debug!(
        event = "index.local_compact.finish",
        db_path = %db_path.display(),
        bytes = new_bytes,
        "index.local_compact.finish"
    );
    Ok(())
}

fn replace_atomic(tmp: &Path, path: &Path) -> std::io::Result<()> {
    match std::fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            // Some platforms (e.g. Windows) do not allow renaming over an existing destination.
            // Avoid deleting the existing DB: move it aside, then restore it if the replace fails.
            let mut backup = path.to_path_buf();
            backup.set_extension(format!("bak-{}", uuid::Uuid::new_v4()));

            std::fs::rename(path, &backup)?;
            match std::fs::rename(tmp, path) {
                Ok(()) => {
                    let _ = std::fs::remove_file(&backup);
                    Ok(())
                }
                Err(e) => {
                    let _ = std::fs::rename(&backup, path);
                    let _ = std::fs::remove_file(tmp);
                    Err(e)
                }
            }
        }
        Err(e) => Err(e),
    }
}

struct UploadedIndex {
    manifest: IndexManifest,
    manifest_object_id: String,
}

struct PendingIndexPart {
    no: u32,
    encrypted: Vec<u8>,
    hash: String,
}

#[allow(clippy::too_many_arguments)]
async fn upload_index_part<S: Storage>(
    storage: &S,
    provider: &str,
    index_id: &str,
    index_upload_sequence: u64,
    part: PendingIndexPart,
    adaptive: &Arc<AdaptiveUploadController>,
    cancel: &CancellationToken,
    rate_limiter: &UploadRateLimiter,
    uploaded_bytes: &AtomicU64,
    uploaded_net_bytes: &AtomicU64,
    have_uploaded_net_bytes: &AtomicBool,
    upload_workload_total: &AtomicU64,
    upload_confirmed_bytes: &AtomicU64,
    progress: Option<&dyn ProgressSink>,
    source_files_total: Option<u64>,
    source_bytes_total: Option<u64>,
    files_indexed: u64,
    chunks_total: u64,
    bytes_read: u64,
    bytes_uploaded_source: u64,
    source_bytes_need_upload_total: u64,
    bytes_deduped: u64,
) -> Result<IndexManifestPart> {
    let queue_started = Instant::now();
    // Keep the shared admission slot until this part has completed all attempts.
    let _slot = adaptive.acquire_slot(cancel).await?;
    let worker_index = _slot.worker_index;
    let queue_wait_ms = queue_started.elapsed().as_millis() as u64;
    let part_len_u64 = part.encrypted.len() as u64;
    adaptive.on_attempt();

    for attempt in 1..=UPLOAD_OBJECT_MAX_ATTEMPTS {
        let rate_limit_wait_ms = rate_limiter.wait_turn().await.as_millis() as u64;
        info!(
            event = "performance.upload.rate_limit_wait",
            kind = "index_part",
            index_upload_sequence,
            index_part_no = part.no,
            attempt,
            worker = worker_index,
            rate_limit_wait_ms,
            "performance.upload.rate_limit_wait"
        );
        let filename = telegram_camouflaged_filename();
        let last_reported = AtomicU64::new(0);
        let last_reported_net = AtomicU64::new(0);
        info!(
            event = "performance.upload.start",
            kind = "index_part",
            index_upload_sequence,
            index_part_no = part.no,
            attempt,
            worker = worker_index,
            payload_bytes = part_len_u64,
            queue_wait_ms,
            rate_limit_wait_ms,
            "performance.upload.start"
        );
        let upload_started = Instant::now();
        let upload_res = storage
            .upload_document_with_progress(
                &filename,
                part.encrypted.clone(),
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
                            source_bytes_need_upload_total: Some(source_bytes_need_upload_total),
                            chunks_total: Some(chunks_total),
                            chunks_done: Some(chunks_total),
                            bytes_read: Some(bytes_read),
                            upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
                            bytes_uploaded_confirmed: Some(
                                upload_confirmed_bytes.load(Ordering::Relaxed),
                            ),
                            bytes_uploaded_source: Some(bytes_uploaded_source),
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
            .await;
        info!(
            event = "performance.upload.finish",
            kind = "index_part",
            index_upload_sequence,
            index_part_no = part.no,
            attempt,
            worker = worker_index,
            payload_bytes = part_len_u64,
            rpc_duration_ms = upload_started.elapsed().as_millis() as u64,
            result = if upload_res.is_ok() {
                "succeeded"
            } else {
                "failed"
            },
            "performance.upload.finish"
        );

        match upload_res {
            Ok(object_id) => {
                let reported = last_reported.load(Ordering::Relaxed);
                if reported < part_len_u64 {
                    uploaded_bytes.fetch_add(part_len_u64 - reported, Ordering::Relaxed);
                }
                upload_confirmed_bytes.fetch_add(part_len_u64, Ordering::Relaxed);
                if let Some(sink) = progress {
                    sink.on_progress(TaskProgress {
                        phase: "index".to_string(),
                        files_total: None,
                        files_done: Some(files_indexed),
                        source_files_total,
                        source_bytes_total,
                        source_bytes_need_upload_total: Some(source_bytes_need_upload_total),
                        chunks_total: Some(chunks_total),
                        chunks_done: Some(chunks_total),
                        bytes_read: Some(bytes_read),
                        upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
                        bytes_uploaded_confirmed: Some(
                            upload_confirmed_bytes.load(Ordering::Relaxed),
                        ),
                        bytes_uploaded_source: Some(bytes_uploaded_source),
                        bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
                        net_bytes_uploaded: have_uploaded_net_bytes
                            .load(Ordering::Relaxed)
                            .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
                        bytes_downloaded: None,
                        net_bytes_downloaded: None,
                        bytes_deduped: Some(bytes_deduped),
                    });
                }
                adaptive.on_success();
                return Ok(IndexManifestPart {
                    no: part.no,
                    size: part.encrypted.len(),
                    hash: part.hash,
                    object_id,
                });
            }
            Err(error) => {
                let reported = last_reported.load(Ordering::Relaxed).min(part_len_u64);
                if reported > 0 {
                    saturating_sub_u64(uploaded_bytes, reported);
                }
                let reported_net = last_reported_net.load(Ordering::Relaxed);
                if reported_net > 0 {
                    saturating_sub_u64(uploaded_net_bytes, reported_net);
                }

                if attempt < UPLOAD_OBJECT_MAX_ATTEMPTS && is_retryable_upload_error(&error) {
                    let backoff = upload_object_retry_backoff(attempt);
                    warn!(
                        event = "io.telegram.upload_retry",
                        provider,
                        kind = "index_part",
                        snapshot_id = index_id,
                        part_no = part.no,
                        blob_bytes = part_len_u64,
                        attempt,
                        max_attempts = UPLOAD_OBJECT_MAX_ATTEMPTS,
                        backoff_ms = backoff.as_millis() as u64,
                        error = %error,
                        "io.telegram.upload_retry"
                    );
                    let retry_wait_started = Instant::now();
                    sleep(backoff).await;
                    info!(
                        event = "performance.upload.retry_wait",
                        kind = "index_part",
                        index_upload_sequence,
                        index_part_no = part.no,
                        attempt,
                        worker = worker_index,
                        retry_wait_ms = retry_wait_started.elapsed().as_millis() as u64,
                        "performance.upload.retry_wait"
                    );
                    continue;
                }

                adaptive.on_failure(&error);
                return Err(Error::Telegram {
                    message: format!(
                        "upload failed: kind=index_part snapshot_id={index_id} part_no={} bytes={part_len_u64}; {error}",
                        part.no
                    ),
                });
            }
        }
    }

    let error = Error::Telegram {
        message: format!(
            "upload failed: kind=index_part snapshot_id={index_id} part_no={} bytes={part_len_u64}; retry loop exhausted",
            part.no
        ),
    };
    adaptive.on_failure(&error);
    Err(error)
}

#[allow(clippy::too_many_arguments)]
async fn upload_index_sqlite_db<S: Storage>(
    storage: &S,
    config: &BackupConfig,
    index_id: &str,
    sqlite_db_path: &Path,
    temp_parent: &Path,
    adaptive_controller: &Arc<AdaptiveUploadController>,
    cancel: &CancellationToken,
    rate_limiter: &UploadRateLimiter,
    uploaded_bytes: &AtomicU64,
    uploaded_net_bytes: &AtomicU64,
    have_uploaded_net_bytes: &AtomicBool,
    upload_workload_total: &AtomicU64,
    upload_confirmed_bytes: &AtomicU64,
    progress: Option<&dyn ProgressSink>,
    source_files_total: Option<u64>,
    source_bytes_total: Option<u64>,
    files_indexed: u64,
    chunks_total: u64,
    bytes_read: u64,
    bytes_uploaded_source: u64,
    source_bytes_need_upload_total: u64,
    bytes_deduped: u64,
) -> Result<UploadedIndex> {
    let provider = storage.provider();
    let index_upload_sequence = INDEX_UPLOAD_SEQUENCE.fetch_add(1, Ordering::Relaxed);

    // Stream-compress the DB into a temp file. This avoids holding multi-GB index
    // databases in process memory.
    let compression_started = Instant::now();
    info!(
        event = "performance.index.compression.start",
        index_upload_sequence, "performance.index.compression.start"
    );
    let compression_result = (|| -> Result<(tempfile::NamedTempFile, u64)> {
        let mut db_file = File::open(sqlite_db_path)?;
        let mut compressed_file = tempfile::Builder::new()
            .prefix("televy-index-upload-")
            .tempfile_in(temp_parent)?;
        {
            let out = compressed_file.as_file_mut();
            out.set_len(0)?;
            out.seek(SeekFrom::Start(0))?;
            let mut encoder = zstd::stream::Encoder::new(out, 0)?;
            std::io::copy(&mut db_file, &mut encoder)?;
            let out = encoder.finish()?;
            out.flush()?;
        }
        let compressed_len = compressed_file.as_file().metadata()?.len();
        Ok((compressed_file, compressed_len))
    })();
    let (compressed_file, compressed_len) = match compression_result {
        Ok(result) => {
            info!(
                event = "performance.index.compression.finish",
                index_upload_sequence,
                duration_ms = compression_started.elapsed().as_millis() as u64,
                compressed_bytes = result.1,
                result = "succeeded",
                "performance.index.compression.finish"
            );
            result
        }
        Err(error) => {
            info!(
                event = "performance.index.compression.finish",
                index_upload_sequence,
                duration_ms = compression_started.elapsed().as_millis() as u64,
                result = "failed",
                "performance.index.compression.finish"
            );
            return Err(error);
        }
    };
    let index_parts = if compressed_len == 0 {
        0
    } else {
        compressed_len.div_ceil(INDEX_PART_BYTES as u64)
    };
    let index_parts_payload_total =
        compressed_len.saturating_add(index_parts.saturating_mul(FRAMING_OVERHEAD_BYTES as u64));
    if index_parts_payload_total > 0 {
        upload_workload_total.fetch_add(index_parts_payload_total, Ordering::Relaxed);
    }

    let max_parallel_parts = config
        .rate_limit
        .max_concurrent_uploads
        .clamp(1, ADAPTIVE_MAX_CONCURRENCY as u32) as usize;
    let mut parts = Vec::new();
    let mut uploads = FuturesUnordered::new();
    let mut first_upload_error = None;
    let mut part_buf = vec![0u8; INDEX_PART_BYTES];
    let mut reader = compressed_file.reopen()?;
    reader.seek(SeekFrom::Start(0))?;
    let mut part_no: u32 = 0;
    loop {
        let mut filled = 0usize;
        while filled < INDEX_PART_BYTES {
            let n = reader.read(&mut part_buf[filled..INDEX_PART_BYTES])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled == 0 {
            break;
        }

        let aad = index_part_aad(index_id, part_no);
        let encrypted = encrypt_framed(&config.master_key, aad.as_bytes(), &part_buf[..filled])?;
        let part = PendingIndexPart {
            no: part_no,
            hash: blake3::hash(&encrypted).to_hex().to_string(),
            encrypted,
        };
        uploads.push(upload_index_part(
            storage,
            provider,
            index_id,
            index_upload_sequence,
            part,
            adaptive_controller,
            cancel,
            rate_limiter,
            uploaded_bytes,
            uploaded_net_bytes,
            have_uploaded_net_bytes,
            upload_workload_total,
            upload_confirmed_bytes,
            progress,
            source_files_total,
            source_bytes_total,
            files_indexed,
            chunks_total,
            bytes_read,
            bytes_uploaded_source,
            source_bytes_need_upload_total,
            bytes_deduped,
        ));
        part_no = part_no.saturating_add(1);

        if uploads.len() >= max_parallel_parts {
            match uploads.next().await.expect("index upload future exists") {
                Ok(part) => parts.push(part),
                Err(error) => {
                    first_upload_error = Some(error);
                    break;
                }
            }
        }
    }
    while let Some(part) = uploads.next().await {
        match part {
            Ok(part) => parts.push(part),
            Err(error) if first_upload_error.is_none() => first_upload_error = Some(error),
            Err(_) => {}
        }
    }
    if let Some(error) = first_upload_error {
        return Err(error);
    }
    parts.sort_by_key(|part| part.no);

    let manifest = IndexManifest {
        version: 1,
        snapshot_id: index_id.to_string(),
        hash_alg: "blake3".to_string(),
        enc_alg: "xchacha20poly1305".to_string(),
        compression: "zstd".to_string(),
        parts,
    };
    let manifest_json = serde_json::to_vec(&manifest).map_err(|_| Error::InvalidConfig {
        message: "serialize index manifest failed".to_string(),
    })?;

    let manifest_enc = encrypt_framed(&config.master_key, index_id.as_bytes(), &manifest_json)?;
    let manifest_bytes = manifest_enc.len() as u64;
    upload_workload_total.fetch_add(manifest_bytes, Ordering::Relaxed);
    let manifest_queue_started = Instant::now();
    // The manifest shares the same admission cap as data and index parts.
    let _manifest_slot = adaptive_controller.acquire_slot(cancel).await?;
    let manifest_worker = _manifest_slot.worker_index;
    let manifest_queue_wait_ms = manifest_queue_started.elapsed().as_millis() as u64;
    adaptive_controller.on_attempt();
    let mut manifest_object_id: Option<String> = None;
    for attempt in 1..=UPLOAD_OBJECT_MAX_ATTEMPTS {
        let rate_limit_wait_ms = rate_limiter.wait_turn().await.as_millis() as u64;
        info!(
            event = "performance.upload.rate_limit_wait",
            kind = "index_manifest",
            index_upload_sequence,
            attempt,
            worker = manifest_worker,
            rate_limit_wait_ms,
            "performance.upload.rate_limit_wait"
        );
        let manifest_filename = telegram_camouflaged_filename();
        let last_reported = AtomicU64::new(0);
        let last_reported_net = AtomicU64::new(0);
        info!(
            event = "performance.upload.start",
            kind = "index_manifest",
            index_upload_sequence,
            attempt,
            worker = manifest_worker,
            payload_bytes = manifest_bytes,
            queue_wait_ms = manifest_queue_wait_ms,
            rate_limit_wait_ms,
            "performance.upload.start"
        );
        let upload_started = Instant::now();
        let upload_res = storage
            .upload_document_with_progress(
                &manifest_filename,
                manifest_enc.clone(),
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
                            source_bytes_need_upload_total: Some(source_bytes_need_upload_total),
                            chunks_total: Some(chunks_total),
                            chunks_done: Some(chunks_total),
                            bytes_read: Some(bytes_read),
                            upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
                            bytes_uploaded_confirmed: Some(
                                upload_confirmed_bytes.load(Ordering::Relaxed),
                            ),
                            bytes_uploaded_source: Some(bytes_uploaded_source),
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
            .await;
        info!(
            event = "performance.upload.finish",
            kind = "index_manifest",
            index_upload_sequence,
            attempt,
            worker = manifest_worker,
            payload_bytes = manifest_bytes,
            rpc_duration_ms = upload_started.elapsed().as_millis() as u64,
            result = if upload_res.is_ok() {
                "succeeded"
            } else {
                "failed"
            },
            "performance.upload.finish"
        );

        match upload_res {
            Ok(uploaded_manifest_object_id) => {
                let reported = last_reported.load(Ordering::Relaxed);
                if reported < manifest_bytes {
                    uploaded_bytes.fetch_add(manifest_bytes - reported, Ordering::Relaxed);
                }
                adaptive_controller.on_success();
                manifest_object_id = Some(uploaded_manifest_object_id);
                break;
            }
            Err(e) => {
                let reported = last_reported.load(Ordering::Relaxed).min(manifest_bytes);
                if reported > 0 {
                    saturating_sub_u64(uploaded_bytes, reported);
                }
                let reported_net = last_reported_net.load(Ordering::Relaxed);
                if reported_net > 0 {
                    saturating_sub_u64(uploaded_net_bytes, reported_net);
                }

                if attempt < UPLOAD_OBJECT_MAX_ATTEMPTS && is_retryable_upload_error(&e) {
                    let backoff = upload_object_retry_backoff(attempt);
                    warn!(
                        event = "io.telegram.upload_retry",
                        provider,
                        kind = "index_manifest",
                        snapshot_id = index_id,
                        blob_bytes = manifest_bytes,
                        attempt,
                        max_attempts = UPLOAD_OBJECT_MAX_ATTEMPTS,
                        backoff_ms = backoff.as_millis() as u64,
                        error = %e,
                        "io.telegram.upload_retry"
                    );
                    let retry_wait_started = Instant::now();
                    sleep(backoff).await;
                    info!(
                        event = "performance.upload.retry_wait",
                        kind = "index_manifest",
                        index_upload_sequence,
                        attempt,
                        worker = manifest_worker,
                        retry_wait_ms = retry_wait_started.elapsed().as_millis() as u64,
                        "performance.upload.retry_wait"
                    );
                    continue;
                }

                error!(
                    event = "io.telegram.upload_failed",
                    provider,
                    snapshot_id = index_id,
                    kind = "index_manifest",
                    blob_bytes = manifest_bytes,
                    attempts = attempt,
                    error = %e,
                    "io.telegram.upload_failed"
                );
                let error = Error::Telegram {
                    message: format!(
                        "upload failed: kind=index_manifest snapshot_id={index_id} bytes={manifest_bytes}; {e}"
                    ),
                };
                adaptive_controller.on_failure(&error);
                return Err(error);
            }
        }
    }
    let manifest_object_id = manifest_object_id.ok_or_else(|| Error::Telegram {
        message: format!(
            "upload failed: kind=index_manifest snapshot_id={index_id} bytes={manifest_bytes}; retry loop exhausted"
        ),
    });
    let manifest_object_id = match manifest_object_id {
        Ok(object_id) => object_id,
        Err(error) => {
            adaptive_controller.on_failure(&error);
            return Err(error);
        }
    };

    upload_confirmed_bytes.fetch_add(manifest_bytes, Ordering::Relaxed);
    if let Some(sink) = progress {
        sink.on_progress(TaskProgress {
            phase: "index".to_string(),
            files_total: None,
            files_done: Some(files_indexed),
            source_files_total,
            source_bytes_total,
            source_bytes_need_upload_total: Some(source_bytes_need_upload_total),
            chunks_total: Some(chunks_total),
            chunks_done: Some(chunks_total),
            bytes_read: Some(bytes_read),
            upload_bytes_total: Some(upload_workload_total.load(Ordering::Relaxed)),
            bytes_uploaded_confirmed: Some(upload_confirmed_bytes.load(Ordering::Relaxed)),
            bytes_uploaded_source: Some(bytes_uploaded_source),
            bytes_uploaded: Some(uploaded_bytes.load(Ordering::Relaxed)),
            net_bytes_uploaded: have_uploaded_net_bytes
                .load(Ordering::Relaxed)
                .then_some(uploaded_net_bytes.load(Ordering::Relaxed)),
            bytes_downloaded: None,
            net_bytes_downloaded: None,
            bytes_deduped: Some(bytes_deduped),
        });
    }

    Ok(UploadedIndex {
        manifest,
        manifest_object_id,
    })
}

async fn persist_snapshot_remote_index_meta(
    conn: &mut DbConn,
    provider: &str,
    snapshot_id: &str,
    uploaded: &UploadedIndex,
) -> Result<()> {
    for part in &uploaded.manifest.parts {
        execute_sqlite_with_busy_retry!(
            "remote_index_parts.upsert",
            sqlx::query(
                r#"
                INSERT OR REPLACE INTO remote_index_parts (snapshot_id, part_no, provider, object_id, size, hash)
                VALUES (?, ?, ?, ?, ?, ?)
                "#,
            )
            .bind(snapshot_id)
            .bind(part.no as i64)
            .bind(provider)
            .bind(&part.object_id)
            .bind(part.size as i64)
            .bind(&part.hash)
            .execute(&mut **conn)
        )?;
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
        .bind(&uploaded.manifest_object_id)
        .execute(&mut **conn)
    )?;

    Ok(())
}

fn path_to_utf8(path: &Path) -> Result<String> {
    path.to_str()
        .map(|s| s.to_string())
        .ok_or_else(|| Error::NonUtf8Path {
            path: path.to_path_buf(),
        })
}

fn file_metadata_values(metadata: &std::fs::Metadata) -> (i64, i64, i64) {
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
}

#[derive(Debug, PartialEq, Eq)]
enum BaseCopyRevalidation {
    Match,
    Changed { size: i64, mtime_ms: i64, mode: i64 },
    NotFound,
    NotFile,
}

fn revalidate_file_for_base_copy(
    path: &Path,
    expected_size: i64,
    expected_mtime_ms: i64,
    expected_mode: i64,
) -> std::io::Result<BaseCopyRevalidation> {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() => {
            let (size, mtime_ms, mode) = file_metadata_values(&metadata);
            if (size, mtime_ms, mode) == (expected_size, expected_mtime_ms, expected_mode) {
                Ok(BaseCopyRevalidation::Match)
            } else {
                Ok(BaseCopyRevalidation::Changed {
                    size,
                    mtime_ms,
                    mode,
                })
            }
        }
        Ok(_) => Ok(BaseCopyRevalidation::NotFile),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(BaseCopyRevalidation::NotFound)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io;

    use ignore::Error as IgnoreError;
    use sqlx::Row;

    use super::{
        BaseCopyRevalidation, BaseFileChunkCopyRow, SCAN_TRACE_BUCKET_US, SCAN_TRACE_MAX_BUCKETS,
        ScanActivityTrace, ScanPerformance, ScanSqliteRetryWait, ScanWorkKind, attach_db,
        delete_transient_scan_file, error_has_flood_wait, export_endpoint_index_db_for_upload,
        file_metadata_values, ignore_error_is_non_root_not_found, initialize_base_chunk_copy_map,
        insert_filemap_chunks_batch, materialize_base_chunk_copy_map,
        revalidate_file_for_base_copy, seed_base_snapshot_chunks, stage_base_chunk_copy_batch,
    };
    use crate::Error;

    #[test]
    fn scan_trace_coarsens_during_collection() {
        let mut trace = ScanActivityTrace::new(std::time::Instant::now());
        let start_us = SCAN_TRACE_BUCKET_US * SCAN_TRACE_MAX_BUCKETS as u64 * 10;
        trace.record_interval_us(ScanWorkKind::Walk, start_us, start_us + 1_000);

        assert!(
            trace.buckets.len() <= SCAN_TRACE_MAX_BUCKETS,
            "scan trace collection must remain bounded"
        );

        let (json, resolution_ms) = trace.to_json(BTreeMap::new(), BTreeMap::new());
        assert!(resolution_ms > 1_000);
        let trace_json: serde_json::Value = serde_json::from_str(&json).unwrap();
        let buckets = trace_json["buckets"].as_array().unwrap();
        assert!(buckets.len() <= SCAN_TRACE_MAX_BUCKETS);
        assert_eq!(
            buckets
                .iter()
                .filter_map(|bucket| bucket["walk_us"].as_u64())
                .sum::<u64>(),
            1_000
        );
    }

    #[test]
    fn base_copy_revalidation_rejects_a_file_removed_after_collection() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("transient.txt");
        std::fs::write(&path, b"before collection").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let (size, mtime_ms, mode) = file_metadata_values(&metadata);

        std::fs::remove_file(&path).unwrap();

        assert_eq!(
            revalidate_file_for_base_copy(&path, size, mtime_ms, mode).unwrap(),
            BaseCopyRevalidation::NotFound
        );
    }

    #[test]
    fn base_copy_revalidation_rejects_a_file_that_becomes_a_directory() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("changed-type");
        std::fs::create_dir(&path).unwrap();

        assert_eq!(
            revalidate_file_for_base_copy(&path, 1, 0, 0).unwrap(),
            BaseCopyRevalidation::NotFile
        );
    }

    #[test]
    fn scan_trace_counts_a_retried_sqlite_operation_once() {
        let scan_started = std::time::Instant::now();
        let mut performance = ScanPerformance::new(scan_started);
        let retry_started = std::time::Instant::now();
        let retry_waits = [ScanSqliteRetryWait {
            started: retry_started,
            finished: std::time::Instant::now(),
        }];

        performance.record_sqlite("files.insert", scan_started, &retry_waits);

        let (trace_json, _) = performance.trace_json();
        let trace: serde_json::Value = serde_json::from_str(&trace_json).unwrap();
        assert_eq!(trace["sqlite_ops_count"]["files.insert"], 1);
    }

    #[tokio::test]
    async fn transient_scan_file_cleanup_removes_the_filemap_row() {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("filemap.sqlite");
        let pool = crate::index_db::open_index_db(&db_path).await.unwrap();
        let mut conn = pool.acquire().await.unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES ('s1', '2026-01-01T00:00:00Z', '/source', 'test', NULL)",
        )
        .execute(&mut *conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES ('f1', 's1', 'transient.txt', 1, 0, 0, 'file')",
        )
        .execute(&mut *conn)
        .await
        .unwrap();

        delete_transient_scan_file(&mut conn, "f1").await.unwrap();

        let rows: i64 = sqlx::query("SELECT COUNT(*) AS n FROM files WHERE file_id = 'f1'")
            .fetch_one(&mut *conn)
            .await
            .unwrap()
            .get("n");
        assert_eq!(rows, 0);
    }

    #[tokio::test]
    async fn filemap_chunk_batch_writes_deduplicated_rows() {
        let temp = tempfile::tempdir().unwrap();
        let pool = crate::index_db::open_index_db(&temp.path().join("filemap.sqlite"))
            .await
            .unwrap();
        let mut conn = pool.acquire().await.unwrap();
        let rows = vec![
            super::FilemapChunkRow {
                chunk_hash: "chunk-1".to_string(),
                size: 10,
            },
            super::FilemapChunkRow {
                chunk_hash: "chunk-1".to_string(),
                size: 10,
            },
            super::FilemapChunkRow {
                chunk_hash: "chunk-2".to_string(),
                size: 20,
            },
        ];

        let retry_waits = insert_filemap_chunks_batch(&mut conn, &rows).await.unwrap();
        assert!(retry_waits.is_empty());
        let count: i64 = sqlx::query("SELECT COUNT(*) AS n FROM chunks")
            .fetch_one(&mut *conn)
            .await
            .unwrap()
            .get("n");
        assert_eq!(count, 2);
    }

    #[tokio::test]
    async fn base_copy_seeds_chunks_once_and_batches_file_chunk_rows() {
        let temp = tempfile::tempdir().unwrap();
        let base_path = temp.path().join("base.sqlite");
        let filemap_path = temp.path().join("filemap.sqlite");

        let base_pool = crate::index_db::open_index_db(&base_path).await.unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES ('base', '2026-01-01T00:00:00Z', '/source', 'base', NULL)",
        )
        .execute(&base_pool)
        .await
        .unwrap();
        for (file_id, path) in [("base-file-1", "one"), ("base-file-2", "two")] {
            sqlx::query(
                "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES (?, 'base', ?, 1, 0, 0, 'file')",
            )
            .bind(file_id)
            .bind(path)
            .execute(&base_pool)
            .await
            .unwrap();
        }
        for (hash, file_id, seq) in [
            ("chunk-1", "base-file-1", 0i64),
            ("chunk-2", "base-file-1", 1),
            ("chunk-2", "base-file-2", 0),
        ] {
            sqlx::query(
                "INSERT OR IGNORE INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES (?, 1, 'blake3', 'xchacha20poly1305', '2026-01-01T00:00:00Z')",
            )
            .bind(hash)
            .execute(&base_pool)
            .await
            .unwrap();
            sqlx::query(
                "INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len) VALUES (?, ?, ?, ?, 1)",
            )
            .bind(file_id)
            .bind(seq)
            .bind(hash)
            .bind(seq)
            .execute(&base_pool)
            .await
            .unwrap();
        }
        drop(base_pool);

        let filemap_pool = crate::index_db::open_index_db(&filemap_path).await.unwrap();
        let mut filemap_conn = filemap_pool.acquire().await.unwrap();
        attach_db(&mut filemap_conn, "base", &base_path)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES ('current', '2026-01-02T00:00:00Z', '/source', 'current', 'base')",
        )
        .execute(&mut *filemap_conn)
        .await
        .unwrap();
        for (file_id, path) in [("current-file-1", "one"), ("current-file-2", "two")] {
            sqlx::query(
                "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES (?, 'current', ?, 1, 0, 0, 'file')",
            )
            .bind(file_id)
            .bind(path)
            .execute(&mut *filemap_conn)
            .await
            .unwrap();
        }

        seed_base_snapshot_chunks(&mut filemap_conn, "base")
            .await
            .unwrap();
        let mut rows = vec![
            BaseFileChunkCopyRow {
                file_id: "current-file-1".to_string(),
                base_file_id: "base-file-1".to_string(),
                size: 1,
            },
            BaseFileChunkCopyRow {
                file_id: "current-file-2".to_string(),
                base_file_id: "base-file-2".to_string(),
                size: 1,
            },
        ];
        initialize_base_chunk_copy_map(&mut filemap_conn)
            .await
            .unwrap();
        let mut second_batch = rows.split_off(1);
        let retry_waits = stage_base_chunk_copy_batch(&mut filemap_conn, &mut rows)
            .await
            .unwrap();
        let second_retry_waits = stage_base_chunk_copy_batch(&mut filemap_conn, &mut second_batch)
            .await
            .unwrap();
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM file_chunks")
                .fetch_one(&mut *filemap_conn)
                .await
                .unwrap(),
            0
        );
        let (copied_chunks, materialize_retry_waits) =
            materialize_base_chunk_copy_map(&mut filemap_conn)
                .await
                .unwrap();
        assert_eq!(copied_chunks, 3);
        assert!(retry_waits.is_empty());
        assert!(second_retry_waits.is_empty());
        assert!(materialize_retry_waits.is_empty());
        assert!(rows.is_empty());

        let chunks: i64 = sqlx::query("SELECT COUNT(*) AS n FROM chunks")
            .fetch_one(&mut *filemap_conn)
            .await
            .unwrap()
            .get("n");
        let file_chunks: i64 = sqlx::query("SELECT COUNT(*) AS n FROM file_chunks")
            .fetch_one(&mut *filemap_conn)
            .await
            .unwrap()
            .get("n");
        assert_eq!(chunks, 2);
        assert_eq!(file_chunks, 3);
    }

    #[test]
    fn flood_wait_detection_matches_regular_and_premium() {
        assert!(error_has_flood_wait(&Error::Telegram {
            message: "rpc error: FLOOD_WAIT_12".to_string(),
        }));
        assert!(error_has_flood_wait(&Error::Telegram {
            message: "rpc error: FLOOD_PREMIUM_WAIT_34".to_string(),
        }));

        // Some errors include "flood wait" in a human-readable form.
        assert!(error_has_flood_wait(&Error::Telegram {
            message: "rpc error 420: flood wait (value: 5)".to_string(),
        }));
        assert!(error_has_flood_wait(&Error::Telegram {
            message: "rpc error 420: flood premium wait (value: 5)".to_string(),
        }));

        assert!(!error_has_flood_wait(&Error::Telegram {
            message: "AUTH_KEY_UNREGISTERED".to_string(),
        }));
    }

    #[test]
    fn non_root_not_found_without_path_is_skipped_only_when_root_exists() {
        let temp = tempfile::tempdir().unwrap();
        let root_exists = temp.path().join("source");
        std::fs::create_dir_all(&root_exists).unwrap();
        let root_missing = temp.path().join("missing");

        let non_root_not_found = IgnoreError::WithDepth {
            depth: 1,
            err: Box::new(IgnoreError::Io(io::Error::from(io::ErrorKind::NotFound))),
        };
        let root_not_found = IgnoreError::WithDepth {
            depth: 0,
            err: Box::new(IgnoreError::Io(io::Error::from(io::ErrorKind::NotFound))),
        };

        assert!(ignore_error_is_non_root_not_found(
            &non_root_not_found,
            &root_exists,
        ));
        assert!(!ignore_error_is_non_root_not_found(
            &non_root_not_found,
            &root_missing,
        ));
        assert!(!ignore_error_is_non_root_not_found(
            &root_not_found,
            &root_exists,
        ));
    }

    #[tokio::test]
    async fn endpoint_index_export_excludes_file_maps() {
        let dir = tempfile::tempdir().unwrap();
        let source_db = dir.path().join("source.sqlite");

        let pool = crate::index_db::open_index_db(&source_db).await.unwrap();

        // Two sources, two snapshots each.
        for (snapshot_id, created_at, source_path) in [
            ("snp_a1", "2026-01-01T00:00:00Z", "/a"),
            ("snp_a2", "2026-01-02T00:00:00Z", "/a"),
            ("snp_b1", "2026-01-01T00:00:00Z", "/b"),
            ("snp_b2", "2026-01-03T00:00:00Z", "/b"),
        ] {
            sqlx::query(
                "INSERT INTO snapshots (snapshot_id, created_at, source_path, label, base_snapshot_id) VALUES (?, ?, ?, 'test', NULL)",
            )
            .bind(snapshot_id)
            .bind(created_at)
            .bind(source_path)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO remote_indexes (snapshot_id, provider, manifest_object_id, created_at) VALUES (?, 'telegram.mtproto/test', ?, ?)",
            )
            .bind(snapshot_id)
            .bind(format!("man_{snapshot_id}"))
            .bind(created_at)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO remote_index_parts (snapshot_id, part_no, provider, object_id, size, hash) VALUES (?, 0, 'telegram.mtproto/test', ?, 1, 'h')",
            )
            .bind(snapshot_id)
            .bind(format!("part_{snapshot_id}"))
            .execute(&pool)
            .await
            .unwrap();
        }

        // One file per snapshot; only the latest per source should remain in the export.
        for (snapshot_id, file_id, chunk_hash) in [
            ("snp_a1", "f_a1", "chk_a1"),
            ("snp_a2", "f_a2", "chk_a2"),
            ("snp_b1", "f_b1", "chk_b1"),
            ("snp_b2", "f_b2", "chk_b2"),
        ] {
            sqlx::query(
                "INSERT INTO files (file_id, snapshot_id, path, size, mtime_ms, mode, kind) VALUES (?, ?, 'x.txt', 1, 0, 0, 'file')",
            )
            .bind(file_id)
            .bind(snapshot_id)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO chunks (chunk_hash, size, hash_alg, enc_alg, created_at) VALUES (?, 1, 'blake3', 'xchacha20poly1305', '2026-01-01T00:00:00Z')",
            )
            .bind(chunk_hash)
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO chunk_objects (chunk_hash, provider, object_id, created_at) VALUES (?, 'telegram.mtproto/test', ?, '2026-01-01T00:00:00Z')",
            )
            .bind(chunk_hash)
            .bind(format!("tgfile:obj_{chunk_hash}"))
            .execute(&pool)
            .await
            .unwrap();

            sqlx::query(
                "INSERT INTO file_chunks (file_id, seq, chunk_hash, offset, len) VALUES (?, 0, ?, 0, 1)",
            )
            .bind(file_id)
            .bind(chunk_hash)
            .execute(&pool)
            .await
            .unwrap();
        }

        // Add a task row to ensure we keep it.
        sqlx::query(
            "INSERT INTO tasks (task_id, kind, state, created_at, started_at, finished_at, snapshot_id, error_code, error_message) VALUES ('t1', 'backup', 'done', '2026-01-01T00:00:00Z', NULL, NULL, 'snp_a1', NULL, NULL)",
        )
        .execute(&pool)
        .await
        .unwrap();

        drop(pool);

        let (exported_db, _stats) =
            export_endpoint_index_db_for_upload(&source_db, dir.path(), true)
                .await
                .unwrap();

        let export_pool = crate::index_db::open_existing_index_db(exported_db.path())
            .await
            .unwrap();

        let snapshots: i64 = sqlx::query("SELECT COUNT(*) AS n FROM snapshots")
            .fetch_one(&export_pool)
            .await
            .unwrap()
            .get("n");
        assert_eq!(snapshots, 4);

        // Endpoint DB export must not include file maps (`files` / `file_chunks`).
        let files: i64 = sqlx::query("SELECT COUNT(*) AS n FROM files")
            .fetch_one(&export_pool)
            .await
            .unwrap()
            .get("n");
        assert_eq!(files, 0);

        let file_chunks: i64 = sqlx::query("SELECT COUNT(*) AS n FROM file_chunks")
            .fetch_one(&export_pool)
            .await
            .unwrap()
            .get("n");
        assert_eq!(file_chunks, 0);

        let tasks: i64 = sqlx::query("SELECT COUNT(*) AS n FROM tasks")
            .fetch_one(&export_pool)
            .await
            .unwrap()
            .get("n");
        assert_eq!(tasks, 1);

        // Global/dedupe state must remain.
        let chunks: i64 = sqlx::query("SELECT COUNT(*) AS n FROM chunks")
            .fetch_one(&export_pool)
            .await
            .unwrap()
            .get("n");
        assert_eq!(chunks, 4);
        let chunk_objects: i64 = sqlx::query("SELECT COUNT(*) AS n FROM chunk_objects")
            .fetch_one(&export_pool)
            .await
            .unwrap()
            .get("n");
        assert_eq!(chunk_objects, 4);

        // When remote dedupe is enabled, endpoint DB exports should exclude dedupe tables too.
        let (exported_db, _stats) =
            export_endpoint_index_db_for_upload(&source_db, dir.path(), false)
                .await
                .unwrap();
        let export_pool = crate::index_db::open_existing_index_db(exported_db.path())
            .await
            .unwrap();
        let chunks: i64 = sqlx::query("SELECT COUNT(*) AS n FROM chunks")
            .fetch_one(&export_pool)
            .await
            .unwrap()
            .get("n");
        assert_eq!(chunks, 0);
        let chunk_objects: i64 = sqlx::query("SELECT COUNT(*) AS n FROM chunk_objects")
            .fetch_one(&export_pool)
            .await
            .unwrap()
            .get("n");
        assert_eq!(chunk_objects, 0);
    }
}
