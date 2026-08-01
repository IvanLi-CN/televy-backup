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

## Retention

After a terminal run, TelevyBackup may prune only completed files that match the
run-log name contract and contain `run.finish`. It deletes files older than the configured age first,
then deletes the remaining oldest files until managed run-log bytes are within
the configured cap. `ui.log`, unknown files, the current run log, and files
held by another process are never candidates. A temporary over-cap state caused
by active files is retried after a later task finishes.

## Performance events

Normal-level run logs include compact `performance.*` records for actual scan
work, actual upload RPC attempts and their waits, and index compression. They
use opaque upload sequence values rather than source paths, chunk hashes, or
remote object IDs so later Gantt charts can join interval endpoints without
disclosing additional file-level data.
