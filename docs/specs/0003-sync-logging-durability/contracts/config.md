# Logging Configuration Contract

## Local file

Path: `<config-dir>/local.toml`

```toml
version = 1

[logging]
level = "normal"

[logging.retention]
max_total_gib = 5
max_age_days = 30
```

`level` accepts `normal`, `verbose`, or `debug`. Missing and invalid local
configuration resolve to `normal`. Writes must be atomic. This file is local to
the machine and is excluded from Backup Config export/import.

`max_total_gib` accepts an integer from `1` through `100`; `max_age_days`
accepts an integer from `7` through `365`. Missing retention fields resolve to
`5 GiB` and `30 days`. An invalid local retention configuration disables run-log
pruning until corrected; it never broadens deletion to other log files.

Before writing either logging setting, the CLI checks a responsive daemon's
`logging.status` response for its additive retention fields. If those fields
are absent, it rejects the write as daemon-incompatible and requires an app
restart. With no reachable daemon, the local setting is written normally.

## Environment

- `TELEVYBACKUP_LOG`: tracing `EnvFilter` expression and highest precedence.
- `RUST_LOG`: tracing `EnvFilter` expression, below `TELEVYBACKUP_LOG` and above
  the local preference.
- `TELEVYBACKUP_LOG_DIR`: run-log directory override.

An invalid filter expression resolves to the `Normal` preset and is reported as
an invalid override; it must not fall back to debug.

## Presets

- `Normal`: global `warn`, TelevyBackup crate targets `info`.
- `Verbose`: global `info`, TelevyBackup crate targets `debug`.
- `Debug`: global `debug`.

## Privacy

Configuration and log records must not contain tokens, master keys, secret
payloads, or values read from Keychain.
