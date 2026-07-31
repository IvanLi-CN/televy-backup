# Implementation

## Current State

- Per-run NDJSON creation, unique naming, flush, and fsync are implemented.
- `local.toml` version 1 stores the machine-local log preset with atomic writes
  and safe `Normal` fallback.
- The shared resolver implements environment precedence, preset mappings, and
  invalid-filter fallback without enabling global debug.
- The run logger reloads its filter between tasks. The daemon keeps its active
  runtime filter stable and reports pending local changes over `logging.status`.
- CLI diagnostics commands expose local/runtime state and best-effort disk use.
- macOS Settings includes Diagnostics controls, override locking, persistent
  Debug warning, and mock-only visual evidence scenes.

## Validation Coverage

- Rust unit tests cover parsing, defaults, atomic persistence, precedence,
  invalid filters, NDJSON output, and SQLx filtering across preset reloads.
- Daemon and CLI tests cover runtime status, pending state, and daemon fallback.
- Swift tests cover status decoding, picker locking, pending state, and Debug
  warning visibility.
- CI-equivalent formatting, lint, Rust test, and Swift test commands are the
  required release gate.

## Migration State

- Canonical spec established from legacy plan #0003.
- `pending delete approval=docs/plan/0003:sync-logging-durability/`
