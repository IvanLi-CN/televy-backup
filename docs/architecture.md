# Architecture: TelevyBackup

## Components

- **GUI app**: native macOS app (SwiftUI/AppKit; built via `scripts/macos/*`).
  - Provides Settings UI and task controls (backup/restore/verify).
  - Spawns the local `televybackup` CLI for long-running operations and streams progress from stdout.
- **Core library**: `televy_backup_core` (`crates/core/`).
  - Implements scan → CDC chunking → hash → encrypt framing → enqueue uploads → worker uploads → SQLite index.
  - Backup pipeline is phase-split (scan/upload/index); scan enqueues jobs into a bounded queue and upload workers honor endpoint rate limits.
  - Implements restore/verify using remote index manifest + chunk downloads.
- **Daemon**: `televybackupd` (`crates/daemon/`).
  - Runs scheduled backups (hourly/daily) and applies retention policy.
  - Intended to be managed by `brew services` as a user-level LaunchAgent.
  - Owns all secrets access (Keychain / `vault.key` / `secrets.enc`). Other components must use daemon IPC.

## Status snapshots (Popover / Developer dashboard)

The macOS popover “dashboard” UI is driven by a single snapshot schema (`StatusSnapshot`) and a single stream:

- **Source of truth** (daemon): local IPC status stream (Unix domain socket).
  - Socket: `$TELEVYBACKUP_DATA_DIR/ipc/status.sock` (or macOS default data dir when env vars are unset).
  - Semantics:
    - `generatedAt` is used for stale detection in the UI.
    - `global.*Total` and `targets[].upTotal` are **session totals** (UI/stream start → now) and are not persisted.
- **Fallback** (daemon → file): `status.json` written by `televybackupd` via atomic write + rename.
  - Path: `$TELEVYBACKUP_DATA_DIR/status/status.json`.
- **Transport** (CLI): `televybackup --json status stream` emits NDJSON, one `status.snapshot` per line.
  - The UI runs this as a long-lived process and decodes each line.
  - The UI should pass `TELEVYBACKUP_CONFIG_DIR` / `TELEVYBACKUP_DATA_DIR` to the spawned CLI so it connects to the same IPC socket as the daemon.
  - If IPC is unavailable, the CLI falls back to reading `status.json`; if both are unavailable, it returns `status.unavailable`.
  - If the CLI binary itself is unavailable (dev/local), the UI may fall back to polling `status.json` directly at low frequency (e.g. 1Hz) to avoid a blank dashboard.

## Daemon control IPC (settings/secrets boundary)

In addition to the status stream socket, there is a separate daemon “control plane” socket:

- Socket: `<TELEVYBACKUP_DATA_DIR>/ipc/control.sock`
- Purpose: allow the CLI and macOS app to query **presence/status** and trigger **write actions** (e.g. ensuring vault
  key availability, updating secrets) without directly accessing Keychain / `vault.key` / `secrets.enc`.
- Security posture: the control IPC must not return vault key plaintext; access is scoped by Unix socket file
  permissions.

## Daemon vault IPC (vault/keychain operations)

The daemon also exposes a dedicated “vault” socket used by the CLI/macOS app to access vault/keychain operations via
daemon-only boundary:

- Socket: `<TELEVYBACKUP_DATA_DIR>/ipc/vault.sock`
- Purpose: allow other components to request “vault key get-or-create” and limited Keychain actions without directly
  linking to Keychain APIs.
- Security posture: must not expose the vault key plaintext; access is scoped by Unix socket file permissions.

## Data locations

The app and daemon can share the same data locations via env vars:

- `TELEVYBACKUP_CONFIG_DIR`: config directory (contains `config.toml`)
- `TELEVYBACKUP_DATA_DIR`: data directory (contains `index/index.<endpoint_id>.sqlite`)
- `TELEVYBACKUP_LOG_DIR`: override per-run log directory (defaults to `TELEVYBACKUP_DATA_DIR/logs/`)

When env vars are not set, the GUI uses `~/Library/Application Support/TelevyBackup`.

Per-run logs are written to files as NDJSON and never mixed into stdout/stderr, so `televybackup --events` stdout remains NDJSON-only and stderr remains error-JSON-only.

The macOS GUI also writes an append-only UI log file `ui.log` into the same log directory (best effort; redacts `api.telegram.org` URL segments).

## Daemon lifecycle (auto-start expectation)

The UI dashboard is best-effort without the daemon, but “live” status requires `televybackupd` to be running and writing `status.json`.

Expected behavior:

- When the user opens the popover, the app should make a best-effort attempt to ensure the daemon is running (so `status.json` begins updating quickly).
  - Preferred: `launchctl kickstart` the user LaunchAgent if present (Homebrew services label `homebrew.mxcl.televybackupd`).
  - Fallback (dev/local): spawn a bundled `televybackupd` if available.
- When the user clicks `Backup now` in the popover header, the app triggers an immediate backup wave for all `enabled=true` targets by writing a control file:
  - Path: `$TELEVYBACKUP_DATA_DIR/control/backup-now`
  - The daemon polls for this trigger and consumes it (best-effort remove + run).

Implementation options:

- **LaunchAgent (recommended)**: install/manage `televybackupd` via `launchd` (e.g. Homebrew services).
  - The UI can optionally “kickstart” the LaunchAgent when opening the popover.
  - Pros: standard macOS background-process model; stable; avoids multiple daemon instances.
- **Bundle-and-spawn**: embed `televybackupd` inside the `.app` bundle and spawn it from the UI.
  - Pros: fewer external setup steps.
  - Cons: requires bundling/updates for the daemon binary; careful lifecycle/dup prevention; entitlements/signing considerations.

## Secrets (vault key + local secrets store)

Secrets are not stored in `config.toml`.

### Daemon-only boundary

Keychain / `vault.key` / `secrets.enc` are daemon-only:

- `televybackupd` is the only component that may read/write the vault key backend and decrypt/update `secrets.enc`.
- The CLI (`televybackup`) and macOS app must treat secrets as remote state and use daemon control IPC.

### Production default (Keychain)

- Keychain (macOS): vault key `televybackup.vault_key` (Base64 32 bytes)
  - Used to encrypt/decrypt the local secrets store.
- Local secrets store: `TELEVYBACKUP_CONFIG_DIR/secrets.enc`
  - Telegram bot token: entry key = `[[telegram_endpoints]].bot_token_key` (per-endpoint)
  - Master key: entry key = `televybackup.master_key` (Base64 32 bytes)
  - MTProto API hash: entry key = `telegram.mtproto.api_hash` (default; key name configurable via `telegram.mtproto.api_hash_key`)
  - MTProto session: entry key = `[[telegram_endpoints]].mtproto.session_key` (per-endpoint; Base64)

### Development bypass (disable Keychain; security downgrade)

For development only, the daemon can be configured to avoid any Keychain access:

- `TELEVYBACKUP_DISABLE_KEYCHAIN=1`
- Vault key file:
  - Default: `TELEVYBACKUP_CONFIG_DIR/vault.key`
  - Override: `TELEVYBACKUP_VAULT_KEY_FILE=<path>`

This is a security downgrade because `vault.key` is persisted on disk.

Master key portability:

- CLI can export/import a human-transferable recovery string `TBK1:<base64url_no_pad>` (aka “gold key”).
- CLI can export/import an encrypted config bundle key `TBC2:<base64url_no_pad>` (Settings v2 + required secrets + passphrase-protected `TBK1`).

### Config bundle (TBC2)

The config bundle is a single copy/paste key used to restore a working setup on a new device:

- **Self-contained**: includes `TBK1` (master key), but importing a `TBC2:...` key requires a user-supplied passphrase.
- **Encrypted**:
  - `TBK1` is framed-encrypted with a passphrase-derived key (PBKDF2-HMAC-SHA256; random salt) using AAD `televy.config.bundle.v2.gold_key`.
  - Bundle payload plaintext is JSON and is framed-encrypted with the master key using AAD `televy.config.bundle.v2.payload`.
- **Secrets coverage**: exports only the secrets referenced by Settings (e.g. bot tokens, MTProto api_hash); MTProto session secrets are intentionally excluded.
- **Import flow**:
  - Dry-run: decode + inspect + preflight (source path existence, pinned bootstrap/catalog, remote latest pointers, local index match).
  - Apply: requires explicit confirmation and can rebuild per-endpoint index DB from remote latest (or initialize an empty DB when bootstrap is missing).

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

Remote-first index sync (backup preflight):

- `backup run` treats the pinned catalog’s `latest` remote index as the **source of truth**.
- Before entering `scan`, it may download the remote latest index DB (manifest → parts → decrypt → zstd → SQLite) and atomically replace `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`.
  - If the pinned catalog is missing: skip sync (first backup / no cross-device pointer).
  - If the pinned catalog exists but cannot be decrypted: fail with `bootstrap.decrypt_failed` (do not overwrite pinned).
  - Can be disabled for offline/debug via `backup run --no-remote-index-sync` (no pinned read; no remote index download).

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
