use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime};

use base64::Engine;
use chrono::{Datelike, Timelike};
use sqlx::Row;
use televy_backup_core::status::{
    Counter, GlobalStatus, Progress, Rate, StatusSnapshot, StatusSource, StatusWriteOptions,
    TargetRunSummary, TargetState, now_unix_ms, status_ipc_socket_path, status_json_path,
    write_status_snapshot_json_atomic_with_options,
};
use televy_backup_core::{
    BackupConfig, BackupOptions, ChunkingConfig, TelegramMtProtoStorage,
    TelegramMtProtoStorageConfig,
};
use televy_backup_core::{ProgressSink, Storage, TaskProgress};
use televy_backup_core::{bootstrap, config as settings_config};
use tokio::time::{Duration, sleep};
use uuid::Uuid;

mod status_ipc;

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
}

#[derive(Debug)]
struct StatusRuntimeState {
    target_order: Vec<String>,
    targets: HashMap<String, TargetRuntime>,
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

    fn mark_run_start(&mut self, target_id: &str) {
        let Some(t) = self.targets.get_mut(target_id) else {
            return;
        };
        t.state = "running".to_string();
        t.running_since = Some(now_unix_ms());
        t.progress = Some(Progress {
            phase: "running".to_string(),
            files_total: None,
            files_done: None,
            chunks_total: None,
            chunks_done: None,
            bytes_read: None,
            bytes_uploaded: Some(0),
            bytes_deduped: Some(0),
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
            chunks_total: p.chunks_total,
            chunks_done: p.chunks_done,
            bytes_read: p.bytes_read,
            bytes_uploaded: p.bytes_uploaded,
            bytes_deduped: p.bytes_deduped,
        });
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
        t.state = "idle".to_string();
        t.running_since = None;
        t.progress = None;
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
        t.state = "failed".to_string();
        t.running_since = None;
        t.progress = None;
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

    fn has_running(&self) -> bool {
        self.targets.values().any(|t| t.state == "running")
    }

    fn build_snapshot(&self, now_ms: u64) -> StatusSnapshot {
        let mut out_targets = Vec::new();
        for id in &self.target_order {
            let Some(t) = self.targets.get(id) else {
                continue;
            };
            out_targets.push(TargetState {
                target_id: t.target_id.clone(),
                label: t.label.clone(),
                source_path: t.source_path.clone(),
                endpoint_id: t.endpoint_id.clone(),
                enabled: t.enabled,
                state: t.state.clone(),
                running_since: t.running_since,
                up: Rate {
                    bytes_per_second: None,
                },
                up_total: Counter { bytes: None },
                progress: t.progress.clone(),
                last_run: t.last_run.clone(),
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
                    bytes_per_second: None,
                },
                down: Rate {
                    bytes_per_second: None,
                },
                up_total: Counter { bytes: None },
                down_total: Counter { bytes: None },
                ui_uptime_seconds: None,
            },
            targets: out_targets,
            extra: Default::default(),
        }
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

async fn status_writer_loop(state: Arc<Mutex<StatusRuntimeState>>, status_path: PathBuf) {
    let mut last_write = Instant::now()
        .checked_sub(Duration::from_secs(3600))
        .unwrap_or_else(Instant::now);

    loop {
        let has_running = state
            .lock()
            .ok()
            .map(|st| st.has_running())
            .unwrap_or(false);

        let now = Instant::now();
        let min_interval = if has_running {
            Duration::from_millis(200)
        } else {
            Duration::from_secs(1)
        };
        let should_write = now.duration_since(last_write) >= min_interval;
        if should_write {
            let snapshot_opt = {
                match state.lock() {
                    Ok(st) => Some(st.build_snapshot(now_unix_ms())),
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

        let tick = if has_running {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(200)
        };
        sleep(tick).await;
    }
}

fn file_mtime(path: &PathBuf) -> Option<SystemTime> {
    std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
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
    let db_path = data_root.join("index").join("index.sqlite");

    let config_path = settings_config::config_path(&config_root);
    let mut settings = settings_config::load_settings_v2(&config_root)?;
    settings_config::validate_settings_schema_v2(&settings)?;
    let mut last_config_mtime = file_mtime(&config_path);

    let status_state = Arc::new(Mutex::new(StatusRuntimeState::from_settings(&settings)));
    let status_path = status_json_path(&data_root);
    tokio::spawn(status_writer_loop(status_state.clone(), status_path));

    let ipc_socket_path = status_ipc_socket_path(&data_root);
    let ipc_state = status_state.clone();
    let _status_ipc_server = match status_ipc::spawn_status_ipc_server(
        ipc_socket_path.clone(),
        Arc::new(move || {
            let now_ms = now_unix_ms();
            match ipc_state.lock() {
                Ok(st) => {
                    let has_running = st.has_running();
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
            tracing::warn!(
                event = "status.ipc_bind_failed",
                error = %e,
                path = %ipc_socket_path.display(),
                "status.ipc_bind_failed"
            );
            None
        }
    };

    let vault_key = load_or_create_vault_key()?;
    let secrets_path = televy_backup_core::secrets::secrets_path(&config_root);
    let mut secrets_store = televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)?;
    let mut last_secrets_mtime = file_mtime(&secrets_path);

    let mut master_key_b64 = get_secret_from_store(&secrets_store, MASTER_KEY_KEY)
        .or_else(|| keychain_get_secret(MASTER_KEY_KEY).ok().flatten())
        .ok_or("master key missing")?;
    let mut master_key = decode_base64_32(&master_key_b64)?;

    let mut api_hash = get_secret_from_store(&secrets_store, &settings.telegram.mtproto.api_hash_key)
        .ok_or("telegram api_hash missing")?;

    let mut schedule_state_by_target = HashMap::<String, TargetScheduleState>::new();
    let mut storage_by_endpoint = HashMap::<String, TelegramMtProtoStorage>::new();

    let manual_trigger_pending = Arc::new(AtomicBool::new(false));
    tokio::spawn(manual_trigger_watcher_loop(
        data_root.clone(),
        manual_trigger_pending.clone(),
    ));

    loop {
        let now = chrono::Local::now();
        let manual_triggered = manual_trigger_pending.swap(false, Ordering::SeqCst);

        // Hot-reload settings + secrets when files change. This avoids confusing situations where the
        // UI saved new endpoint chat_id but the long-running daemon kept using the old one.
        let has_running = status_state.lock().ok().is_some_and(|st| st.has_running());
        if !has_running {
            let config_mtime = file_mtime(&config_path);
            let secrets_mtime = file_mtime(&secrets_path);
            let config_changed = config_mtime.is_some() && config_mtime != last_config_mtime;
            let secrets_changed = secrets_mtime.is_some() && secrets_mtime != last_secrets_mtime;

            if config_changed || secrets_changed {
                if config_changed {
                    match settings_config::load_settings_v2(&config_root) {
                        Ok(new_settings) => {
                            if let Err(e) = settings_config::validate_settings_schema_v2(&new_settings)
                            {
                                tracing::warn!(
                                    event = "config.reload_failed",
                                    error = %e,
                                    path = %config_path.display(),
                                    "config.reload_failed"
                                );
                            } else {
                                settings = new_settings;
                                last_config_mtime = config_mtime;
                                storage_by_endpoint.clear();
                                schedule_state_by_target.retain(|k, _| {
                                    settings.targets.iter().any(|t| t.id == *k)
                                });
                                if let Ok(mut st) = status_state.lock() {
                                    st.apply_settings(&settings);
                                }
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
                    match televy_backup_core::secrets::load_secrets_store(&secrets_path, &vault_key)
                    {
                        Ok(new_store) => {
                            secrets_store = new_store;
                            last_secrets_mtime = secrets_mtime;
                            storage_by_endpoint.clear();

                            if let Some(v) = get_secret_from_store(&secrets_store, MASTER_KEY_KEY)
                                .or_else(|| keychain_get_secret(MASTER_KEY_KEY).ok().flatten())
                            {
                                master_key_b64 = v;
                                if let Ok(k) = decode_base64_32(&master_key_b64) {
                                    master_key = k;
                                }
                            }
                            if let Some(h) =
                                get_secret_from_store(&secrets_store, &settings.telegram.mtproto.api_hash_key)
                            {
                                api_hash = h;
                            }

                            tracing::info!(
                                event = "secrets.reloaded",
                                path = %secrets_path.display(),
                                "secrets.reloaded"
                            );
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
            }
        }

        if settings.telegram.mtproto.api_id <= 0 || settings.telegram.mtproto.api_hash_key.trim().is_empty() {
            // Keep the daemon alive so the UI can show status, but skip running backups until config is fixed.
            sleep(Duration::from_secs(1)).await;
            continue;
        }

        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        for target in &settings.targets {
            if !target.enabled {
                continue;
            }

            let state = schedule_state_by_target
                .entry(target.id.clone())
                .or_default();

            let scheduled_slot = if manual_triggered {
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

            let Some(ep) = settings
                .telegram_endpoints
                .iter()
                .find(|e| e.id == target.endpoint_id)
            else {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    status = "failed",
                    error_code = "config.invalid",
                    error_message = "target references unknown endpoint_id",
                    target_id = %target.id,
                    endpoint_id = %target.endpoint_id,
                    "run.finish"
                );
                continue;
            };

            if ep.chat_id.trim().is_empty() {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    status = "failed",
                    error_code = "config.invalid",
                    error_message = "endpoint chat_id is empty",
                    target_id = %target.id,
                    endpoint_id = %ep.id,
                    "run.finish"
                );
                continue;
            }

            let bot_token = get_secret_from_store(&secrets_store, &ep.bot_token_key)
                .or_else(|| keychain_get_secret(&ep.bot_token_key).ok().flatten());
            let Some(bot_token) = bot_token else {
                tracing::error!(
                    event = "run.finish",
                    kind = "backup",
                    status = "failed",
                    error_code = "telegram.unauthorized",
                    error_message = "bot token missing",
                    target_id = %target.id,
                    endpoint_id = %ep.id,
                    "run.finish"
                );
                continue;
            };

            if !storage_by_endpoint.contains_key(&ep.id) {
                let session = match get_secret_from_store(&secrets_store, &ep.mtproto.session_key) {
                    Some(b64) if !b64.trim().is_empty() => {
                        Some(base64::engine::general_purpose::STANDARD.decode(b64.as_bytes())?)
                    }
                    _ => None,
                };

                let cache_dir = data_root.join("cache").join("mtproto").join(&ep.id);
                std::fs::create_dir_all(&cache_dir)?;
                let provider = settings_config::endpoint_provider(&ep.id);

                let storage = TelegramMtProtoStorage::connect(TelegramMtProtoStorageConfig {
                    provider,
                    api_id: settings.telegram.mtproto.api_id,
                    api_hash: api_hash.clone(),
                    bot_token: bot_token.clone(),
                    chat_id: ep.chat_id.clone(),
                    session,
                    cache_dir,
                    helper_path: None,
                })
                .await?;

                storage_by_endpoint.insert(ep.id.clone(), storage);
            }

            let storage = match storage_by_endpoint.get(&ep.id) {
                Some(s) => s,
                None => continue,
            };

            // Only consume the schedule slot once all required config/secrets are available
            // and the endpoint storage is ready.
            match scheduled_slot {
                ScheduleSlot::Hourly(key) => state.last_hourly = Some(key),
                ScheduleSlot::Daily(key) => state.last_daily = Some(key),
                ScheduleSlot::Manual => {
                    // If a manual trigger happens to coincide with a scheduled slot, consume that slot too
                    // to avoid an immediate second run within the same minute.
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

            let task_id = format!("tsk_{}", Uuid::new_v4());
            let run_log =
                televy_backup_core::run_log::start_run_log("backup", &task_id, &data_root)?;

            tracing::info!(
                event = "run.start",
                kind = "backup",
                run_id = %task_id,
                task_id = %task_id,
                target_id = %target.id,
                endpoint_id = %ep.id,
                source_path = %target.source_path,
                log_path = %run_log.path().display(),
                "run.start"
            );

            let started = Instant::now();
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

            let cfg = BackupConfig {
                db_path: db_path.clone(),
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
            };

            if let Ok(mut st) = status_state.lock() {
                st.mark_run_start(&target.id);
            }

            let sink = StatusProgressSink {
                target_id: target.id.clone(),
                state: status_state.clone(),
            };
            let opts = BackupOptions {
                cancel: None,
                progress: Some(&sink),
            };

            let result = televy_backup_core::run_backup_with(storage, cfg, opts).await;
            let duration_seconds = started.elapsed().as_secs_f64();

            match result {
                Ok(res) => {
                    tracing::info!(
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

                    if let Ok(mut st) = status_state.lock() {
                        st.mark_run_finish_success(
                            &target.id,
                            duration_seconds,
                            res.files_indexed,
                            res.bytes_uploaded,
                            res.bytes_deduped,
                        );
                    }

                    let pool =
                        televy_backup_core::index_db::open_existing_index_db(&db_path).await?;
                    let row = sqlx::query(
                        "SELECT manifest_object_id FROM remote_indexes WHERE snapshot_id = ? AND provider = ? LIMIT 1",
                    )
                    .bind(&res.snapshot_id)
                    .bind(storage.provider())
                    .fetch_one(&pool)
                    .await?;
                    let manifest_object_id: String = row.get("manifest_object_id");

                    if let Err(e) = bootstrap::update_remote_latest(
                        storage,
                        &master_key,
                        &target.id,
                        &target.source_path,
                        &label,
                        &res.snapshot_id,
                        &manifest_object_id,
                    )
                    .await
                    {
                        tracing::error!(
                            event = "bootstrap.update_failed",
                            target_id = %target.id,
                            endpoint_id = %ep.id,
                            error = %e,
                            "bootstrap.update_failed"
                        );
                    }
                }
                Err(e) => {
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

                    if let Ok(mut st) = status_state.lock() {
                        st.mark_run_finish_failure(
                            &target.id,
                            duration_seconds,
                            e.code().to_string(),
                        );
                    }
                }
            }

            if let Some(bytes) = storage.session_bytes() {
                let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
                let should_write = secrets_store
                    .get(&ep.mtproto.session_key)
                    .is_none_or(|v| v != b64.as_str());
                if should_write {
                    secrets_store.set(&ep.mtproto.session_key, b64);
                    if let Err(e) = televy_backup_core::secrets::save_secrets_store(
                        &secrets_path,
                        &vault_key,
                        &secrets_store,
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

        sleep(Duration::from_secs(1)).await;
    }
}

async fn manual_trigger_watcher_loop(data_root: PathBuf, pending: Arc<AtomicBool>) {
    let control_dir = data_root.join("control");
    let path = control_dir.join("backup-now");
    loop {
        let removed = tokio::task::spawn_blocking({
            let path = path.clone();
            move || {
                if !path.exists() {
                    return Ok(false);
                }
                match std::fs::remove_file(&path) {
                    Ok(()) => Ok(true),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
                    Err(e) => Err(e),
                }
            }
        })
        .await;

        match removed {
            Ok(Ok(true)) => {
                pending.store(true, Ordering::SeqCst);
                tracing::info!(
                    event = "manual.trigger",
                    kind = "backup",
                    path = %path.display(),
                    "manual backup trigger consumed"
                );
            }
            Ok(Ok(false)) => {}
            Ok(Err(e)) => {
                tracing::warn!(
                    event = "manual.trigger_remove_failed",
                    kind = "backup",
                    path = %path.display(),
                    error = %e,
                    "failed to consume manual trigger"
                );
            }
            Err(e) => {
                tracing::warn!(
                    event = "manual.trigger_task_failed",
                    kind = "backup",
                    path = %path.display(),
                    error = %e,
                    "failed to spawn blocking task to consume manual trigger"
                );
            }
        }
        sleep(Duration::from_millis(200)).await;
    }
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

#[cfg(target_os = "macos")]
fn keychain_get_secret(key: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    use security_framework::passwords::{PasswordOptions, generic_password};

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
fn keychain_get_secret(_key: &str) -> Result<Option<String>, Box<dyn std::error::Error>> {
    Err("keychain only supported on macOS".into())
}

#[cfg(target_os = "macos")]
fn keychain_set_secret(key: &str, value: &str) -> Result<(), Box<dyn std::error::Error>> {
    use security_framework::passwords::set_generic_password;
    set_generic_password(televy_backup_core::APP_NAME, key, value.as_bytes())?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn is_keychain_not_found(e: &security_framework::base::Error) -> bool {
    // errSecItemNotFound
    e.code() == -25300
}

#[cfg(target_os = "macos")]
fn load_or_create_vault_key() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    let existing = keychain_get_secret(televy_backup_core::secrets::VAULT_KEY_KEY)?
        .map(|s| s.trim().to_string());

    if let Some(b64) = existing {
        return Ok(televy_backup_core::secrets::vault_key_from_base64(&b64)?);
    }

    let mut bytes = [0u8; 32];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| std::io::Error::other(format!("getrandom failed: {e}")))?;
    let b64 = televy_backup_core::secrets::vault_key_to_base64(&bytes);
    keychain_set_secret(televy_backup_core::secrets::VAULT_KEY_KEY, &b64)?;
    Ok(bytes)
}

#[cfg(not(target_os = "macos"))]
fn load_or_create_vault_key() -> Result<[u8; 32], Box<dyn std::error::Error>> {
    Err("vault key only supported on macOS".into())
}
