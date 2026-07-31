# Implementation

## Current State

- Per-run NDJSON creation, unique naming, flush, and fsync are implemented.
- `TELEVYBACKUP_LOG` and `RUST_LOG` filters are implemented.
- The current implementation still defaults to global `debug` and has no App
  preference or runtime status interface.

## Required Coverage

- Add machine-local logging settings and atomic persistence.
- Add safe preset/filter resolution and reload the filter between daemon runs.
- Add CLI and control-IPC diagnostics status.
- Add the macOS Diagnostics settings surface and deterministic UI demo states.
- Update unit, integration, Swift, documentation, and visual evidence coverage.

## Migration State

- Canonical spec established from legacy plan #0003.
- `pending delete approval=docs/plan/0003:sync-logging-durability/`
