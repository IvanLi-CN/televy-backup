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
