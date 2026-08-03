# Implementation

## Current State

- Per-run NDJSON creation, unique naming, flush, and fsync are implemented.
- `local.toml` requires schema version 1 and stores the machine-local log preset
  with atomic writes and safe `Normal` fallback for missing, unversioned, or
  invalid content, including unknown fields caused by misspelled keys.
- The shared resolver implements environment precedence, preset mappings, and
  invalid-filter fallback without enabling global debug.
- The run logger reloads its filter at every task boundary, including between
  targets in one daemon scheduling pass. The daemon keeps its active runtime
  filter stable and reports pending local changes and current validation errors
  over `logging.status`.
- CLI diagnostics commands expose local/runtime state and best-effort disk use;
  before either logging mutation, they reject an older responsive daemon that
  lacks retention-status fields, requiring an app restart before it can parse
  the new local configuration. Read-only legacy daemon status remains readable
  with safe defaults for the additive retention fields.
- Completed run logs are identified by their accepted filename and a terminal
  `run.finish` record, then retained by age and managed byte cap. The current
  log is fsynced before pruning; active shared locks and all unrecognised files
  are skipped.
- Normal-level performance records separate scan-coroutine lifetime from a
  compressed, actual scan-resource trace. The trace records walk, metadata,
  read-chunk, hashing, encryption, and SQLite usage in one-second buckets; queue,
  direct/pack/index RPC, rate-limit/retry waits, and index compression retain
  their own correlatable intervals. Sub-millisecond measurements accumulate in
  microseconds before the scan-finish summary is rounded to milliseconds, so
  high-volume short operations retain their full measured time. All identifiers
  are opaque and no file or remote-object identifier is logged.
- File-row inserts commit in bounded batches of 512 and base file metadata is
  looked up once per batch. Scan trace version 2 exposes the cumulative time
  and count of `files.insert`, `base.files.lookup`, `base_copy`, and
  `chunks.insert.filemap` and `file_chunks.insert`, so a run can identify
  whether SQLite write, base lookup, or chunk-copy work dominates without
  logging file-level data. The temporary snapshot filemap uses a single-writer
  WAL with scan-time auto-checkpoint and sync disabled; it restores FULL sync
  and checkpoints before the SQLite file is uploaded.
- SQLite busy/locked retry sleep is a separate `sqlite_retry_wait_us` trace
  slice and scan-finish summary value. It is excluded from `sqlite_us` and from
  per-operation SQLite timing, preserving the distinction between database
  work and retry waiting.
- macOS Settings includes Diagnostics controls, override locking, persistent
  Debug warning, daemon-start refresh, a reliable Open in Finder action, and
  editable capacity/age retention controls. Its unframed section layout puts
  the selected level and its effective state first, then makes managed run-log
  usage primary before the retention limits, which save automatically. Mock-only visual
  evidence covers normal, Debug, environment-override, and retention states.
- The CLI updates local logging preferences under a configuration-directory lock,
  preserving independently changed level and retention fields across processes.
  It rejects malformed or unsupported local configuration rather than
  overwriting it with defaults.

## Validation Coverage

- Rust unit tests cover parsing, required schema version, defaults, atomic
  persistence, precedence, invalid filters, NDJSON output, and SQLx filtering
  across preset reloads.
- Daemon and CLI tests cover daemon-owned and external CLI runtime status,
  pending state, and daemon fallback.
- Swift tests cover status decoding, picker locking, pending state, and Debug
  warning visibility for presets and debug-capable custom filters, plus
  retention decoding and logarithmic control mappings. Diagnostics reloads use
  sequence guards so stale asynchronous results cannot replace a newer save or
  daemon-backed refresh, and log-level/retention saves are mutually exclusive
  to preserve both settings.
- Backup pipeline tests verify that Normal NDJSON contains parseable measured
  scan-resource buckets as well as upload-queue, upload-attempt,
  rate-limit-wait, and index-compression event boundaries.
- Upload attempts use a shared slot identifier across data and index work. The
  MTProto adapter relays progress from a blocking helper task back to its async
  future, so concurrent attempt intervals remain observable instead of being
  serialized by a synchronous poll.
- The mock-only capture script permits an isolated second app instance and
  captures only the PID-owned Settings window without enabling the app's timer
  snapshot path or falling back to a full-screen capture. An exit trap reaps the
  isolated app and temporary files on both success and failure.
- CI-equivalent formatting, lint, Rust test, and Swift test commands are the
  required release gate.
- Failed scans retain their measured resource counters and partial trace in
  `performance.scan.finish` / `performance.scan.trace`, so interrupted runs
  remain analyzable without treating elapsed lifecycle time as work time.
- Scan-trace coarsening occurs while collecting measurements, keeping the
  in-memory trace capped at 4,096 buckets even when an operation follows a
  long idle interval.

## Migration State

- Canonical spec established from legacy plan #0003.
- `pending delete approval=docs/plan/0003:sync-logging-durability/`
