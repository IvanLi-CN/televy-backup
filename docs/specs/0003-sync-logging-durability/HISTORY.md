# History

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
