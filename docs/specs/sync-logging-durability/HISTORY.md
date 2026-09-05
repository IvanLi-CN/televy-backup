# History

## Canonical Contract Resolution

- The canonical `contracts/config.md` and `contracts/file-formats.md` remain
  authoritative because they contain the newer logging, retention, and
  diagnostics behavior already confirmed by the current implementation.
- The legacy Plan's contract inventory was copied to this topic for traceable
  coverage. Legacy copies remain under `docs/plan/0003:sync-logging-durability/`
  pending separate delete approval and are not a second canonical source.

## 2026-01-20

Per-run NDJSON logging was introduced with environment-controlled filters and a
global debug default to maximize early development diagnostics.

## 2026-01-23

Run logging was retained while backup scanning and upload execution became
separate pipeline phases.

## 2026-07-31

Production evidence showed approximately 37 GiB of run logs, overwhelmingly
SQLx query debug events. The contract changed to a safe `Normal` default plus a
machine-local App preference. Environment overrides remain authoritative for
advanced operation. Debug intentionally remains uncapped and persistent, so the
App must display an ongoing disk-risk warning.

The legacy plan directory remains present pending explicit delete approval.

Review hardening made the schema version mandatory, preserved current
configuration errors while a task retains its previous runtime filter, and
defined every target start as a task boundary for filter reload. The Settings
surface also refreshes after daemon startup and creates the log directory before
opening it. Unknown local-setting fields are rejected so misspelled keys remain
observable instead of silently selecting a default.

Upgrade hardening distinguishes an incompatible older daemon from an unavailable
daemon, preventing the App from presenting a local-only status as applied at
runtime. Debug-capable custom environment filters receive the same persistent
disk warning as the Debug preset.

External CLI tasks now report their resolved logging state to daemon IPC for the
duration of the task, so Diagnostics reflects the process that is actually
writing the active run log. Settings diagnostics refreshes are sequenced to
discard stale asynchronous completions.

## 2026-08-01

The operational contract now bounds completed run logs by both retention age
and managed disk usage. Pruning is deliberately deferred until a later task
finishes, excludes active and non-run files, and fails closed on malformed local
retention settings.

The same change establishes Normal-level performance intervals around the
concurrent scan/upload pipeline. This replaces phase-boundary inference with
actual upload RPC, queue, rate-limit, retry, and index-compression timing.

Scan coroutine lifetime proved insufficient for resource analysis because it
overlaps the upload pipeline by design. Successful runs therefore preserve a
compressed trace of measured scan work and retain gaps for unmeasured time,
rather than presenting the scan lifecycle as a resource-occupancy interval.

The scan-finish summary accumulates measurements before converting to
milliseconds, preserving high-volume short SQLite and metadata operations that
would otherwise disappear through per-operation rounding.

Logging mutations now reject a responsive older daemon that does not advertise
retention support. This prevents an older strict local-settings parser from
silently falling back after the newer CLI writes a retention section. Completed
run-log inspection also accepts a tail slice that begins inside a UTF-8
character, so a valid terminal event continues to make the file eligible for
usage reporting and pruning.

Diagnostics serializes log-level and retention writes so the atomic local TOML
replacement cannot discard a concurrent user change. Scan telemetry records
content hashing separately from encryption, preserving an exact encryption
interval for later resource timelines.

Failed scans retain their accumulated resource counters and partial trace so
diagnostics can analyze work already performed before the failure.

Scan-trace bucket coarsening now occurs during collection rather than only at
serialization. This keeps the producer memory-bounded for multi-hour scans and
long gaps while preserving accumulated measured work.

Retention limits now save automatically after a short debounce, eliminating the
extra confirmation action for a machine-local, non-destructive preference.

Logging preference mutations now hold a shared configuration-directory lock so
concurrent level and retention saves do not overwrite each other.

Malformed or unsupported local logging configuration remains intact when a
preference mutation is attempted. The mutation fails rather than silently
replacing valid neighboring fields with defaults.

## 2026-08-03

Scan telemetry advanced to version 2 and now reports cumulative SQLite time and
batch counts for file-row writes, base-file lookup, base chunk copy, and file
chunk insertion. File metadata writes and unchanged-file baseline reads are
batched at 512 entries, retaining cancellation and the existing scan/upload
pipeline while removing the prior per-file SQLite commit and lookup pattern.
Files are revalidated before base-copy so a transient file cannot inherit old
chunks after a batch lookup. SQLite busy/locked retry sleep is represented as a
separate wait slice rather than as database work.

The snapshot filemap writer now uses bounded multi-row inserts for file metadata,
file chunks, and newly read chunk rows. Unchanged-file chunk rows are seeded once
from the base snapshot and materialized through a set-based mapping query. Because
the temporary filemap has one writer, its WAL auto-checkpoint and scan-time sync
are deferred; FULL sync and an explicit WAL checkpoint run before index upload.

## 2026-08-04

The scan filemap now keeps bounded 512-row statements inside one transaction and
commits once after successful scan materialization. This removes the durable
commit cost from every metadata batch while preserving cancellation rollback and
the existing base lookup, transient-file, and chunk-copy semantics. Snapshot
filemaps also drop the endpoint-only file-kind index because their primary and
unique indexes cover restore and verify lookups.

The one-time filemap transaction commit is included in the SQLite performance map,
and base-copy deduplicated bytes are published as each staged batch is accounted
for instead of waiting for the end of the scan.
