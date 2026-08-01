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

`performance.scan.start` and `.finish` bound the scan coroutine lifetime, not
resource occupancy. Every scan finish, including a failed scan, is preceded by
`performance.scan.trace` with
a `trace_json` string. The inner JSON has `version`, `resolution_ms`, and
`buckets`; each bucket has an `offset_ms` from scan start plus any measured
`walk_us`, `metadata_us`, `read_chunk_us`, `hash_us`, `encrypt_us`, or
`sqlite_us` values.
Normal runs use one-second buckets; long runs are coarsened to retain at most
4,096 buckets, and `resolution_ms` is authoritative. Missing measurement fields
are zero. Consumers must draw only reported measurement values and must not
fill gaps from the scan lifecycle interval.

`performance.scan.queue_wait.start` and `.finish` use an opaque
`queue_wait_id` to bound each actual upload-queue admission wait. Upload RPC
events continue to use their upload sequence and attempt identifiers.
