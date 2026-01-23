# Architecture: TelevyBackup

## Components

- **GUI app**: native macOS app (SwiftUI/AppKit; built via `scripts/macos/*`).
  - Provides Settings UI and task controls (backup/restore/verify).
  - Spawns the local `televybackup` CLI for long-running operations and streams progress from stdout.
- **Core library**: `televy_backup_core` (`crates/core/`).
  - Implements scan → CDC chunking → hash → encrypt framing → upload → SQLite index.
  - Implements restore/verify using remote index manifest + chunk downloads.
- **Daemon**: `televybackupd` (`crates/daemon/`).
  - Runs scheduled backups (hourly/daily) and applies retention policy.
  - Intended to be managed by `brew services` as a user-level LaunchAgent.

## Data locations

The app and daemon can share the same data locations via env vars:

- `TELEVYBACKUP_CONFIG_DIR`: config directory (contains `config.toml`)
- `TELEVYBACKUP_DATA_DIR`: data directory (contains `index/index.sqlite`)
- `TELEVYBACKUP_LOG_DIR`: override per-run log directory (defaults to `TELEVYBACKUP_DATA_DIR/logs/`)

When env vars are not set, the GUI uses `~/Library/Application Support/TelevyBackup`.

Per-run logs are written to files as NDJSON and never mixed into stdout/stderr, so `televybackup --events` stdout remains NDJSON-only and stderr remains error-JSON-only.

## Secrets (vault key + local secrets store)

Secrets are not stored in `config.toml`.

- Keychain (macOS): vault key `televybackup.vault_key` (Base64 32 bytes)
  - Used to encrypt/decrypt the local secrets store.
- Local secrets store: `TELEVYBACKUP_CONFIG_DIR/secrets.enc`
  - Telegram bot token: entry key = `[[telegram_endpoints]].bot_token_key` (per-endpoint)
  - Master key: entry key = `televybackup.master_key` (Base64 32 bytes)
  - MTProto API hash: entry key = `telegram.mtproto.api_hash` (default; key name configurable via `telegram.mtproto.api_hash_key`)
  - MTProto session: entry key = `[[telegram_endpoints]].mtproto.session_key` (per-endpoint; Base64)

Master key portability:

- CLI can export/import a human-transferable recovery string `TBK1:<base64url_no_pad>` (aka “gold key”).

## Crypto and framing

All binary objects uploaded to Telegram use the same framing:

- `version` (1 byte, `0x01`)
- `nonce` (24 bytes, random)
- `ciphertext_and_tag` (AEAD output)

AEAD: XChaCha20-Poly1305

Framing overhead:

- `1(version) + 24(nonce) + 16(tag) = 41 bytes`

Associated Data (AD):

- Chunk blob: `chunk_hash` (hex UTF-8)
- Index part: `snapshot_id + ":" + part_no` (UTF-8)
- Manifest: `snapshot_id` (UTF-8)

## Storage model (Telegram MTProto)

The storage provider is **MTProto-only**:

- `telegram.mode` is fixed to `"mtproto"`.
- New snapshots persist `provider = "telegram.mtproto/<endpoint_id>"` in the local DB (to avoid cross-endpoint dedup/index pollution).
- Historical snapshots with `provider = "telegram.botapi"` are not supported and require a re-backup.

### MTProto (`telegram.mtproto`)

- Each encrypted chunk/index/manifest is uploaded as a Telegram `document` via MTProto.
- `object_id` is versioned: `tgmtproto:v1:<base64url(json)>` (peer/msgId/docId/accessHash; does not store `file_reference`).
- Downloads refresh `file_reference` by fetching the message by `peer+msgId` and are chunked/resumable via `TELEVYBACKUP_DATA_DIR/cache/mtproto/`.
- Engineered upload limit (to cap memory peaks and failure surface): `MTProtoEngineeredUploadMaxBytes = 128MiB`.
  - Since chunk blobs are framed, the effective cap is `chunking.max_bytes <= 128MiB - 41`.
- Pack sizing defaults:
  - `PACK_MAX_BYTES = 128MiB`
  - `PACK_TARGET_BYTES = 64MiB ± 8MiB` (per-pack jitter)
  - `PACK_MAX_ENTRIES_PER_PACK = 32`

## Remote bootstrap/catalog (pinned)

Cross-device restore (without the old local SQLite) uses a per-endpoint “bootstrap catalog”:

- The catalog plaintext is JSON, encrypted via the same framing using AAD `televy.bootstrap.catalog.v1`.
- The encrypted catalog is uploaded as a Telegram `document`.
- A pinned message in the chat acts as a root pointer to the latest catalog document.
- `restore latest` resolves `snapshot_id + manifest_object_id` from the pinned catalog.

## SQLite index

The local index database schema is defined in:

- `docs/plan/0001:telegram-backup-mvp/contracts/db.md`

Key tables:

- `snapshots`, `files`, `file_chunks`
- `chunks`, `chunk_objects`
- `remote_index_parts`, `remote_indexes`

## Retention policy

`retention.keep_last_snapshots` prunes older snapshots from the local SQLite index only:

- Deletes `snapshots`/`files`/`file_chunks`/`remote_index_*` for old snapshots.
- Does not delete remote chunk objects (no remote GC in MVP).

## Known limitations (MVP)

- No APFS snapshot: backups are best-effort consistent at scan time.
- Restore is not a full remote “search”: cross-device restore depends on the pinned bootstrap catalog, and only provides `latest` pointers recorded there.
- No remote chunk GC: Telegram chat storage can grow over time.
