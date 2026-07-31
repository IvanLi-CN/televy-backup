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
  protocol skew with an older daemon is reported as requiring an app restart.
- macOS Settings includes Diagnostics controls, override locking, persistent
  Debug warning, daemon-start refresh, a reliable Open in Finder action, and
  mock-only visual evidence scenes.

## Validation Coverage

- Rust unit tests cover parsing, required schema version, defaults, atomic
  persistence, precedence, invalid filters, NDJSON output, and SQLx filtering
  across preset reloads.
- Daemon and CLI tests cover daemon-owned and external CLI runtime status,
  pending state, and daemon fallback.
- Swift tests cover status decoding, picker locking, pending state, and Debug
  warning visibility for presets and debug-capable custom filters. Diagnostics
  reloads use sequence guards so stale asynchronous results cannot replace a
  newer save or daemon-backed refresh.
- The mock-only capture script permits an isolated second app instance and
  captures only the PID-owned Settings window without enabling the app's timer
  snapshot path or falling back to a full-screen capture.
- CI-equivalent formatting, lint, Rust test, and Swift test commands are the
  required release gate.

## Migration State

- Canonical spec established from legacy plan #0003.
- `pending delete approval=docs/plan/0003:sync-logging-durability/`
