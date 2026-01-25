# CLI Contracts（IPC migration）

## `televybackup status stream`

- Change: Modify（data source: file → IPC）
- Default: connect IPC (`status.sock`) and forward as NDJSON `status.snapshot`
- Fallback: if IPC unavailable, read `status.json` (if present) and forward
- Failure: if both unavailable, return `status.unavailable`

## `televybackup status get`

- Change: Modify（data source: file → IPC）
- Default: connect IPC and return latest snapshot (single JSON)
- Fallback: same as stream

