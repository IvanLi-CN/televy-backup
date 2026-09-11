# Architecture: TelevyBackup

## Components

- **GUI app**: native macOS app (SwiftUI/AppKit; built via `scripts/macos/*`).
  - Provides Settings UI and task controls (backup/restore/verify).
  - SettingsWindow daemon business calls use the versioned `control.sock` contract exclusively;
    it never launches the CLI or falls back to a second transport. The status dashboard may still
    consume the existing CLI status stream for task progress.
  - Runs as the logged-in user. It has neither root privilege nor Full Disk Access (FDA), and it
    never reads Keychain secrets directly.
- **Core library**: `televy_backup_core` (`crates/core/`).
  - Implements scan → CDC chunking → hash → encrypt framing → enqueue uploads → worker uploads → SQLite index. Filemap statements are bounded to 512 entries and the scan transaction commits once; unchanged-file baseline metadata is resolved once per batch.
  - Backup pipeline is phase-split (scan/upload/index); scan enqueues jobs into a bounded queue and upload workers honor endpoint rate limits.
  - Implements restore/verify using remote index manifest + chunk downloads.
- **Daemon**: `televybackupd` (`crates/daemon/`).
  - Runs scheduled backups (hourly/daily) and applies retention policy.
  - Intended to be managed by `brew services` as a user-level LaunchAgent.
  - Owns all secrets access (Keychain / `vault.key` / `secrets.enc`). Other components must use daemon IPC.
  - Runs as the logged-in user and has neither root privilege nor FDA.
  - Supports a local `daemon.stop` control request. App and CLI use it for graceful cancellation and shutdown; the caller waits for IPC disappearance before treating shutdown as complete.
- **APFS Snapshot Access**: `televybackup-snapshot-access` (`crates/snapshot-helper/`).
  - Runs as a separate user-session `LSUIElement` app (`com.ivan.televybackup.snapshot-access`) embedded inside the single visible `TelevyBackup.app` at `Contents/Library/LoginItems/`. The user grants FDA to this exact nested bundle. Its peer-UID checked Unix socket exposes only configured target IDs, opaque leases, metadata pages, and bounded read streams.
  - It creates snapshots and reads their metadata/content, journaling the exact snapshot UUID and private mount root. It never encrypts or uploads backup data.
  - Registration is owned by `SMAppService.agent` and uses `BundleProgram`; the CLI exposes status and a transactional migration from the old external registration. Scheduled backups use the already registered helper and never prompt for a password. Ordinary main-app releases reuse unchanged helper bytes.
- **APFS Snapshot Mount Helper**: `televybackup-snapshot-mount-helper` (`crates/snapshot-helper/src/bin/`).
  - Runs as root under `com.ivan.televybackup.snapshot-mount-helper`. Its restricted IPC only mounts,
    unmounts, and UUID-cleans leases presented by the Access app; it never opens source files or
    touches Keychain, backup indexes, or network storage.
  - The current validated strict-mode baseline also requires FDA for this exact installed helper
    identity. It requires one administrator-authorized installation/update/uninstall transaction;
    normal and scheduled backups use the already loaded helper without prompts.

## macOS authority model

Strict APFS snapshot mode is the only feature that needs FDA or root. Live-mode backups need neither.

| Component | Runtime authority | Setup action | Prohibited authority |
| --- | --- | --- | --- |
| GUI app | Logged-in user | None | root, FDA, direct Keychain reads |
| CLI | Logged-in user | `sudo` only when explicitly installing, updating, or removing the mount helper | FDA, background root operation |
| `televybackupd` | Logged-in-user LaunchAgent; Keychain in production-like mode | None | root, FDA, direct snapshot mount access |
| Snapshot Access.app | Logged-in user with FDA for the exact embedded app identity | Manual FDA grant after the first layout migration or helper identity change | root, Keychain, encryption, network upload |
| Snapshot Mount Helper | root LaunchDaemon with FDA for the exact installed helper identity | Administrator transaction plus manual FDA grant after identity/path change | source file reads, Keychain, indexes, network |

FDA cannot be inferred from service reachability. Settings identifies the two exact paths and reports
the Access app's observed file-access readiness; strict backups fail closed whenever either
required component cannot complete its operation.

## Status snapshots (Popover / Developer dashboard)

The macOS popover “dashboard” UI is driven by a single snapshot schema (`StatusSnapshot`) and a single stream:

- **Source of truth** (daemon): local IPC status stream (Unix domain socket).
  - Socket: `$TELEVYBACKUP_DATA_DIR/ipc/status.sock` (or macOS default data dir when env vars are unset).
  - Semantics:
    - `generatedAt` is used for stale detection in the UI.
    - `global.*Total` and `targets[].upTotal` are **session totals** (UI/stream start → now) and are not persisted.
    - `targets[].progress.sourceFilesTotal` / `sourceBytesTotal` are best-effort local quick stats gathered during backup `prepare` (metadata-only scan). Fields are optional/additive for backward compatibility.
    - Rate semantics:
      - `bytesPerSecond` rates are derived from **payload** progress counters (`progress.bytesUploaded` / `progress.bytesDownloaded`).
      - `targets[].up.bytesPerSecond` is the daemon's estimate of the upload rate.
      - `global.up.bytesPerSecond` is a best-effort sum across targets (typically only one target runs at a time).
      - `global.down.bytesPerSecond` is the daemon's estimate of the download rate (meaningful for restore/verify; usually `0`/`null` during backup).
      - `targets[].activeTask` is an optional additive declaration of the current `backup`, `restore`, `verify`, or `sync` task and its transfer directions. The macOS menu bar uses this declaration, rather than phase names, rates, or `lastRun`, to determine its current activity.
      - Rates are computed from a rolling 1s window sampled at progress time, with interpolation to avoid "one-tick" spikes when progress updates are coarse.
      - Note: some storage providers may also emit best-effort **wire byte** counters (e.g. MTProto socket bytes) in task progress, but these can get ahead due to kernel buffering and should not be used as the primary "last 1s" bandwidth indicator.
- **Fallback** (daemon → file): `status.json` written by `televybackupd` via atomic write + rename.
  - Path: `$TELEVYBACKUP_DATA_DIR/status/status.json`.
- **Transport** (CLI): `televybackup --json status stream` emits NDJSON, one `status.snapshot` per line.
  - The UI runs this as a long-lived process and decodes each line.
  - The CLI throttles the emitted cadence to **2Hz** (500ms) while running so the UI refresh rate is stable and predictable.
  - The UI should pass `TELEVYBACKUP_CONFIG_DIR` / `TELEVYBACKUP_DATA_DIR` to the spawned CLI so it connects to the same IPC socket as the daemon.
  - If IPC is unavailable, the CLI falls back to reading `status.json`; if both are unavailable, it returns `status.unavailable`.
  - If the CLI binary itself is unavailable (dev/local), the UI may fall back to polling `status.json` directly at low frequency (e.g. 1Hz) to avoid a blank dashboard.

The menu bar keeps a client-memory failure latch for a task failure observed in its current live session. It expires after 10 seconds and is not restored from daemon state, `lastRun`, or application restart. Its optional transfer-rate title is a local `UserDefaults` preference and never belongs to portable configuration.

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

## APFS snapshot consistency

Snapshot consistency is opt-in per APFS Volume UUID (`snapshot_volumes.<uuid>.enabled`). When enabled, a backup fails closed if Snapshot Access cannot probe, create, uniquely identify, or mount the snapshot; it never falls back to the live source directory. The core keeps the logical source path in historical indexes and receives file bytes through bounded Snapshot Access streams. The lease is released immediately after the scan has read all source bytes into the encrypted upload queue.

The user daemon remains the scheduler, Keychain boundary, scanner, encryptor, and uploader. Snapshot Access is the FDA/file-read boundary; the mount helper is a mount-only privileged boundary. Non-APFS volumes, nested mounted volumes, ambiguous `tmutil` ownership, unavailable helper, and pending cleanup are reported as unsupported/blocking states rather than silently producing a best-effort backup. See [the APFS snapshot consistency spec](specs/apfs-snapshot-consistency/SPEC.md), [ADR 0008](adr/0008-apfs-snapshot-access-app.md), [ADR 0009](adr/0009-apfs-snapshot-mount-helper.md), and [ADR 0010](adr/0010-identity-stable-single-product-release.md).

## Data locations

The app and daemon can share the same data locations via env vars:

- `TELEVYBACKUP_CONFIG_DIR`: config directory (contains portable `config.toml` and machine-local `local.toml`)
- `TELEVYBACKUP_DATA_DIR`: data directory (contains `index/index.<endpoint_id>.sqlite`)
- `TELEVYBACKUP_LOG_DIR`: override per-run log directory (defaults to `TELEVYBACKUP_DATA_DIR/logs/`)

When env vars are not set, the GUI uses `~/Library/Application Support/TelevyBackup`.

Per-run logs are written to files as NDJSON and never mixed into stdout/stderr, so `televybackup --events` stdout remains NDJSON-only and stderr remains error-JSON-only.

Performance logs distinguish the scan coroutine lifetime from measured scan
resource slices. Successful scans write a compact trace of walk, metadata,
read-chunk, encryption, and SQLite time at one-second resolution for normal
runs, coarsened only for exceptionally long runs. Trace version 2 also reports
cumulative milliseconds and batch counts for file-row insertion, base-file
lookup, base chunk copy, and file chunk insertion. Timeline tools must render
only measured slices and leave unmeasured gaps visible. SQLite busy/locked
retry sleep is a separate wait slice, not SQLite resource time. Upload-queue
waits and storage RPCs have their own actual interval records.

Run-log filtering resolves in this order: `TELEVYBACKUP_LOG`, `RUST_LOG`, the
machine-local Diagnostics preference, then the safe `Normal` default. The daemon
keeps one filter for an active task and applies preference changes before the
next task. `local.toml` is deliberately excluded from Backup Config.

Completed run logs use a separate machine-local retention policy: the default
is `5 GiB` or `30 days`, whichever is exceeded first. Pruning runs only after a
backup, restore, or verify reaches a terminal state, and it only considers
unlocked `sync-*.ndjson` files. The append-only `ui.log` and unknown files are
outside this policy.

Before modifying either logging setting, the CLI checks a responsive daemon for
retention-status support. A daemon from before this contract must be restarted;
the CLI refuses the write instead of leaving that daemon unable to parse the
new local retention section.

The macOS GUI also writes an append-only UI log file `ui.log` into the same log directory (best effort; redacts `api.telegram.org` URL segments).

## Daemon lifecycle (auto-start expectation)

The UI dashboard is best-effort without the daemon, but “live” status requires `televybackupd` to be running and writing `status.json`.

Expected behavior:

- When the user opens the popover, the app should make a best-effort attempt to ensure the daemon is running (so `status.json` begins updating quickly).
  - Preferred: `launchctl kickstart` the product-managed user LaunchAgent if installed (`com.ivan.televybackup.daemon`); detect the Homebrew label only as a legacy fallback.
  - Fallback (dev/local): spawn a bundled `televybackupd` if available.
- When the user clicks `Backup now` in the popover header, the app triggers an immediate backup wave for all `enabled=true` targets by writing a control file:
  - Path: `$TELEVYBACKUP_DATA_DIR/control/backup-now`
  - The daemon polls for this trigger and consumes it (best-effort remove + run).

Implementation options:

- **LaunchAgent (recommended)**: install/manage `televybackupd` via the product CLI and `launchd` (`com.ivan.televybackup.daemon`). Existing Homebrew services remain a compatibility path.
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
  - Dry-run: decode + inspect + preflight (source path existence, pinned bootstrap/catalog, remote latest pointers).
  - Apply: requires explicit confirmation and rebuilds per-endpoint index DB from the pinned remote latest (or initializes an empty DB when bootstrap is missing).
  - Directory selection (rebind):
    - Rebinding a target while remote latest exists is a **data-plane decision**.
    - The UI first runs a **content-level compare** between local folder bytes and the remote latest snapshot:
      - Downloads the remote index DB (manifest + parts) from Telegram.
      - Uses the snapshot’s `file_chunks(offset,len)` + BLAKE3 to validate local files **without using any local index DB** as proof.
    - If the folder is fully identical to remote latest, no prompt is shown.
    - If differences exist, the user must choose a resolution:
      - **Use remote latest**: restore remote latest into the chosen folder (folder must be empty).
      - **Keep local folder**: keep local bytes; a future backup may overwrite remote latest.
      - **Merge (Option B)**: run a backup immediately after import to update remote latest from local bytes (local -> remote new snapshot).
  - Note: local index DBs are not authoritative during import. Import/compare never “proves equality” using a local index, and import never updates the remote pin based on any local index; remote changes must flow through a backup that computes a new snapshot from the actual filesystem.

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

Snapshot Storage inspection uses the local `storage_objects` table alongside
`chunk_objects`. A successful physical upload records the normalized provider/object pair, a
stable opaque `sto_...` identifier, `direct` or `pack` kind, the exact encrypted document payload
bytes, and the record timestamp. The endpoint index is used when remote dedupe is disabled; the
materialized dedupe index is used when it is enabled. Legacy mappings without a row remain readable
but expose unknown document bytes/time. This is an offline, selected-snapshot view: it never queries
Telegram for document or message metadata and never returns raw Telegram locator fields.

## Remote bootstrap/catalog (pinned)

Cross-device restore (without the old local SQLite) uses a per-endpoint “bootstrap catalog”:

- The catalog plaintext is JSON, encrypted via the same framing using AAD `televy.bootstrap.catalog.v1`.
- The encrypted catalog is uploaded as a Telegram `document`.
- A pinned message in the chat acts as a root pointer to the latest catalog document.
- `restore latest` resolves `snapshot_id + manifest_object_id` from the pinned catalog.

Remote-first index sync (backup preflight):

- `backup run` treats the pinned catalog’s `latest` remote index as the **source of truth**.
- Before entering `scan`, backup runs a parallel `prepare` stage:
  - `index_sync`: may download the remote latest index DB (manifest → parts → decrypt → zstd → SQLite) and atomically replace `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`.
  - `local_quick_stats`: metadata-only local walk to estimate source file count/bytes for UI progress denominator.
  - If the pinned catalog is missing: skip sync (first backup / no cross-device pointer).
  - If the pinned catalog exists but cannot be decrypted: fail with `bootstrap.decrypt_failed` (do not overwrite pinned).
  - If local quick stats fails: continue backup with degraded (indeterminate) progress until totals are available.
  - Can be disabled for offline/debug via `backup run --no-remote-index-sync` (no pinned read; no remote index download).

Backup runtime progress model:

- Only `prepare` is rendered as indeterminate.
- Runtime phases `scan` / `scan_upload` / `upload` / `index` are determinate and use a monotonic layered bar:
  - `NeedUploadConfirmed <= UploadingCurrent <= BackedUp <= Scanned`
  - `Scanned`: source traversal/read progress.
  - `BackedUp`: source bytes already protected (`uploaded_source + deduped`).
  - `UploadingCurrent`: in-flight uploaded payload in discovered upload workload.
  - `NeedUploadConfirmed`: confirmed uploaded payload in discovered upload workload.
- Need-upload labels are scope-aware:
  - `Need Upload (Disc.)` while scan is still discovering upload set.
  - `Need Upload (Final)` after scan is complete (`upload`/`index`).

Index publish memory model:

- During `index` phase, SQLite is compressed with streaming zstd into a temporary file and then uploaded in fixed-size encrypted parts.
- Index parts are submitted through the same bounded upload slots as data work;
  their manifest is submitted only after every part succeeds.

The per-snapshot filemap is assembled in a single-writer WAL connection. File
metadata and chunk rows use bounded multi-row writes inside one scan transaction,
and unchanged base chunks are seeded once before set-based `file_chunks` mapping.
Scan-time sync and WAL auto-checkpoint are deferred; FULL sync and an explicit
checkpoint complete before the filemap is uploaded.
- The process does **not** use whole-file `fs::read + encode_all` for index publish, to keep daemon memory bounded on large index databases.

## SQLite index

The local index database schema is defined in:

- `docs/specs/telegram-backup-mvp/contracts/db.md`

Key tables:

- `snapshots`, `files`, `file_chunks`
- `chunks`, `chunk_objects`
- `storage_objects` (write-time physical document metadata)
- `remote_index_parts`, `remote_indexes`

## Retention policy

`retention.keep_last_snapshots` prunes older snapshots from the local SQLite index only:

- Deletes `snapshots`/`files`/`file_chunks`/`remote_index_*` for old snapshots.
- Does not delete remote chunk objects (no remote GC in MVP).

This snapshot retention is distinct from the machine-local run-log retention
described above.

## Known limitations (MVP)

- Snapshot consistency is opt-in per APFS volume; disabled volumes retain the existing live-directory behavior. Enabled volumes never fall back to a live scan after a snapshot precondition fails.
- Restore is not a full remote “search”: cross-device restore depends on the pinned bootstrap catalog, and only provides `latest` pointers recorded there.
- No remote chunk GC: Telegram chat storage can grow over time.
