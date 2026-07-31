# Logging Configuration Contract

## Local file

Path: `<config-dir>/local.toml`

```toml
version = 1

[logging]
level = "normal"
```

`level` accepts `normal`, `verbose`, or `debug`. Missing and invalid local
configuration resolve to `normal`. Writes must be atomic. This file is local to
the machine and is excluded from Backup Config export/import.

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
