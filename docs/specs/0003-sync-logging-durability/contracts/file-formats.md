# Per-run Log File Contract

## Directory

Resolution order:

1. `TELEVYBACKUP_LOG_DIR`
2. `TELEVYBACKUP_DATA_DIR/logs/`
3. `~/Library/Application Support/TelevyBackup/logs/` on macOS

## Name and format

Each run creates `sync-<kind>-<started-at-utc>-<run-id>.ndjson`, where `kind` is
`backup`, `restore`, or `verify`. Every UTF-8 line is a JSON object with at least
`timestamp`, `level`, `target`, and `fields`.

## Durability

Normal run completion flushes and fsyncs the file before returning. Process
crashes are best effort; `SIGKILL` cannot guarantee final records.

No retention, rotation, compression, or size cap is defined by this contract.
