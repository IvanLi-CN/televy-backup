# Sync Logging Durability and Local Diagnostics (#0003)

## Background

Each backup, restore, and verify run writes a standalone NDJSON log. The original
default enabled global debug logging, which also persisted every SQLx query and
allowed normal installations to accumulate tens of gigabytes of logs.

The logging system must remain useful for diagnosis without making dependency
debug output the default. Users must be able to select the intended verbosity
from the macOS app and see when an environment variable overrides that choice.

## Goals

- Keep one parseable NDJSON file per run and flush plus fsync it before return.
- Default existing and new installations to a safe `Normal` preset.
- Persist a machine-local `Normal`, `Verbose`, or `Debug` preference outside the
  portable backup configuration.
- Apply a changed preference to the next run without restarting the daemon.
- Preserve advanced environment-filter overrides and expose their effective
  source in the app.
- Keep secrets and machine-readable CLI output out of run logs.

## Non-goals

- Log rotation, retention, compression, per-file caps, or total-size caps.
- Automatic deletion of existing logs.
- Remote log collection or a general metrics/tracing platform.
- Including the local logging preference in Backup Config export/import.

## Requirements

### Run files

- Every `backup`, `restore`, and `verify` run creates a unique
  `sync-<kind>-<timestamp>-<run-id>.ndjson` file.
- Every line is a UTF-8 JSON object containing `timestamp`, `level`, `target`,
  and `fields`.
- Normal completion flushes and fsyncs the file. Abnormal termination remains
  best effort.
- Logging never writes secrets and never contaminates stdout/stderr protocol
  output.

### Local preference

- `<config-dir>/local.toml` uses schema version 1 and stores
  `[logging] level = "normal|verbose|debug"`.
- A missing or invalid local file resolves safely to `Normal` and reports the
  validation problem without enabling debug.
- Writes are validated and atomic.
- Backup Config serialization remains unchanged and excludes `local.toml`.

### Filter resolution

Precedence is fixed:

1. `TELEVYBACKUP_LOG`
2. `RUST_LOG`
3. the local App preference
4. `Normal`

Preset filters are fixed:

- `Normal`: dependencies at `warn`; TelevyBackup targets at `info`.
- `Verbose`: dependencies at `info`; TelevyBackup targets at `debug`.
- `Debug`: global `debug`, including SQLx query events.

Invalid environment filters resolve to `Normal`, never global debug.

### Runtime behavior

- A run keeps one filter for its entire lifetime.
- A daemon preference change made during a run is reported as pending and is
  applied once the daemon is idle, before the next run starts.
- CLI one-shot runs resolve the same preference and environment precedence
  before creating their run log.

### Diagnostics interfaces

- JSON CLI commands expose `diagnostics get` and
  `diagnostics set-log-level --level <normal|verbose|debug>`.
- Diagnostic status includes configured and effective levels, the effective
  filter and source, the overriding variable when present, a pending level,
  log directory, and best-effort log byte count.
- Daemon control IPC exposes its actual runtime logging status.
- The macOS Settings window includes a `Diagnostics` section with the preset
  picker, actual effective state, log directory/size, and an Open Logs action.
- When an environment variable overrides the preference, the picker is disabled
  and the variable name is visible.
- Effective Debug displays a persistent disk-usage warning without a
  confirmation dialog and remains enabled until changed.

## Contracts

- [Logging configuration](contracts/config.md)
- [Per-run log files](contracts/file-formats.md)

## Acceptance Criteria

- With no environment variables or local file, SQLx debug events are absent and
  TelevyBackup info events are present.
- With Debug configured, SQLx debug events are present.
- Changing the preference during a run does not alter that file; the next run
  uses the new filter.
- Environment overrides are reflected by daemon status and disable the App
  picker.
- An invalid environment expression or local file cannot enable global debug.
- Backup Config export contains no local logging preference.
- All generated run-log lines remain valid NDJSON and run summaries remain
  visible under restrictive filters.

## Visual Evidence

PR: include
![Diagnostics Normal](./assets/diagnostics-normal.png)

PR: include
![Diagnostics Debug warning](./assets/diagnostics-debug.png)

PR: include
![Diagnostics environment override](./assets/diagnostics-override.png)

The images come from deterministic, mock-only Settings `ui_demo` scenes and are
limited to the Settings window launched for the capture.

## References

- Legacy source retained pending delete approval:
  `docs/plan/0003:sync-logging-durability/`
