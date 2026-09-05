# TelevyBackup

macOS desktop backup app + Rust backend (work in progress).

## Prerequisites

- Rust (stable)
- Xcode Command Line Tools (for macOS GUI, `xcrun`)

## Development

- Build CLI/daemon: `cargo build`
- Build macOS app (prod): `./scripts/macos/build-app.sh`
- Build macOS app (dev variant): `TELEVYBACKUP_APP_VARIANT=dev ./scripts/macos/build-app.sh`
- Run macOS app (dev default: Keychain disabled): `./scripts/macos/run-app.sh`
- Run macOS app (prod-like: Keychain enabled): `TELEVYBACKUP_APP_VARIANT=prod ./scripts/macos/run-app.sh`

### macOS app variants (prod vs dev)

To avoid conflicts with an installed Release app on the same machine, local development uses a separate app variant:

- **Prod app**
  - Name: `TelevyBackup`
  - Bundle ID: `com.ivan.televybackup`
  - Default vault key backend: **Keychain enabled**
  - Local build output path: `target/macos-app/TelevyBackup.app`
- **Dev app**
  - Name: `TelevyBackup Dev`
  - Bundle ID: `com.ivan.televybackup.dev`
  - Default vault key backend: **Keychain disabled** (override with `TELEVYBACKUP_DISABLE_KEYCHAIN=0`)
  - Local build output path: `target/macos-app/TelevyBackup Dev.app`

Note: `scripts/macos/run-app.sh` will warn if you start the prod variant with `TELEVYBACKUP_DISABLE_KEYCHAIN=1`.

### Terminology used in troubleshooting ("release"/"dev")

When an agent is asked to "restart release version" on a dev machine, this repo uses:

- **Release** = the **prod variant** (bundle id `com.ivan.televybackup`) started by:
  - `TELEVYBACKUP_APP_VARIANT=prod ./scripts/macos/run-app.sh`
- **Dev** = the **dev variant** (bundle id `com.ivan.televybackup.dev`) started by:
  - `./scripts/macos/run-app.sh` (default)

Important:
- An installed Release app under `/Applications` may have the same prod bundle id, so **do not** guess by app name.
- Always restart via `./scripts/macos/run-app.sh` unless you explicitly want to target an installed app bundle by path.

### How to confirm which app bundle is running

Confirm by **process path** (prod vs dev local builds):

```bash
pgrep -fl 'target/macos-app/TelevyBackup\\.app/Contents/MacOS/TelevyBackup' || true
pgrep -fl 'target/macos-app/TelevyBackup Dev\\.app/Contents/MacOS/TelevyBackup' || true
```

Confirm by **bundle id** (installed or local):

```bash
APP=/Applications/TelevyBackup.app
/usr/bin/mdls -name kMDItemCFBundleIdentifier -name kMDItemVersion "$APP"
```

## Development: bypass Keychain (codesign + vault key)

There are two separate “Keychain touchpoints” during development:

1) **Build-time codesign** (the build script may query Keychain for signing identities)
2) **Runtime vault key** (the daemon normally reads/writes the vault key via Keychain to decrypt `secrets.enc`)

### Build-time: ad-hoc signing (no identity lookup)

Force ad-hoc signing by setting `TELEVYBACKUP_CODESIGN_IDENTITY=-`:

```bash
TELEVYBACKUP_CODESIGN_IDENTITY=- ./scripts/macos/build-app.sh
```

or:

```bash
TELEVYBACKUP_CODESIGN_IDENTITY=- ./scripts/macos/run-app.sh
```

### Runtime: disable Keychain for the daemon (security downgrade)

The **dev app variant** defaults to `TELEVYBACKUP_DISABLE_KEYCHAIN=1`. In this mode, the daemon will **not** access
Keychain and will use a local vault key file instead:

- Default: `TELEVYBACKUP_CONFIG_DIR/vault.key`
- Override: `TELEVYBACKUP_VAULT_KEY_FILE=<path>`

Example:

```bash
TELEVYBACKUP_DISABLE_KEYCHAIN=1 televybackupd
```

Important: `vault.key` on disk is a **security downgrade**. Treat it like a secret and only use this mode for local dev.

To enable Keychain (production-like), run:

```bash
TELEVYBACKUP_DISABLE_KEYCHAIN=0 ./scripts/macos/run-app.sh
```

To run the **prod app variant** (Keychain enabled by default), run:

```bash
TELEVYBACKUP_APP_VARIANT=prod ./scripts/macos/run-app.sh
```

### Daemon-only boundary (secrets)

Keychain / `vault.key` / `secrets.enc` are **daemon-only**:

- `televybackupd` is the only component that may read/write the vault key backend (Keychain or `vault.key`) and decrypt
  `secrets.enc`.
- The CLI (`televybackup`) and macOS app must not access Keychain / `vault.key` / `secrets.enc` directly; use daemon IPC
  (see `docs/architecture.md`).

## Configuration

TelevyBackup stores non-secret settings in `config.toml`, and secrets in an encrypted local secrets store (`secrets.enc`).

- Production default: macOS Keychain stores **only** the vault key used to decrypt `secrets.enc`.
- Development optional: set `TELEVYBACKUP_DISABLE_KEYCHAIN=1` to store the vault key in `vault.key` (security downgrade).

- Telegram storage is **MTProto-only** (`telegram.mode = "mtproto"`). Telegram Bot API is no longer supported; older `telegram.botapi` snapshots require a new backup.
- `config.toml` schema is **v2** (`version = 2`) and supports multiple backup targets and multiple Telegram endpoints:
  - `[[targets]]` (one directory per target) references an `endpoint_id`
  - `[[telegram_endpoints]]` (one endpoint per chat/bot) provides `chat_id` plus secret key names (`bot_token_key`, `mtproto.session_key`)

- `config.toml` location: `TELEVYBACKUP_CONFIG_DIR/config.toml` (default: `~/Library/Application Support/TelevyBackup/config.toml`)
- Machine-local preferences: `TELEVYBACKUP_CONFIG_DIR/local.toml`. Logging level and completed run-log retention are available under **Settings → Diagnostics** and are not included in Backup Config export/import.
- `secrets.enc` location: `TELEVYBACKUP_CONFIG_DIR/secrets.enc` (default: `~/Library/Application Support/TelevyBackup/secrets.enc`)
- Per-endpoint local index DB: `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`
  - Legacy (migration): `TELEVYBACKUP_DATA_DIR/index/index.sqlite` may exist but is ignored and auto-cleaned when all in-use per-endpoint DBs are usable.
- Per-run logs (NDJSON): `TELEVYBACKUP_LOG_DIR/` (override) or `TELEVYBACKUP_DATA_DIR/logs/` (default: `~/Library/Application Support/TelevyBackup/logs/`)
  - Log level filter: `TELEVYBACKUP_LOG` → `RUST_LOG` → `local.toml` → `Normal`
  - `Normal` records TelevyBackup at info and dependencies at warn; `Verbose` raises TelevyBackup to debug and dependencies to info; `Debug` enables global debug and may consume substantial disk space.
  - Completed `sync-*.ndjson` logs default to `5 GiB` or `30 days`, whichever threshold is reached first. Cleanup occurs after a later backup, restore, or verify finishes; `ui.log` is not managed by this policy.
- UI logs (macOS app): `TELEVYBACKUP_LOG_DIR/ui.log` (override) or `TELEVYBACKUP_DATA_DIR/logs/ui.log` (default: `~/Library/Application Support/TelevyBackup/logs/ui.log`)
- Keychain:
  - Vault key: key = `televybackup.vault_key` (Base64 32 bytes)
- Secrets store entries (inside `secrets.enc`):
  - Telegram bot token (used for MTProto bot sign-in): key = `[[telegram_endpoints]].bot_token_key` (per-endpoint)
  - Master key: key = `televybackup.master_key` (Base64 32 bytes)
  - MTProto API hash: key = `telegram.mtproto.api_hash` (default; key name configurable via `telegram.mtproto.api_hash_key`)
  - MTProto session: key = `[[telegram_endpoints]].mtproto.session_key` (per-endpoint; Base64)

### Target ignore rules (`.televyignore`)

Backup target scanning supports gitignore-style rules via `.televyignore` files:

- Place `.televyignore` in a target source root and/or any subdirectory.
- Rules use gitignore semantics (`#` comments, `*`, `**`, `?`, `/` anchoring, trailing `/` for directories, `!` re-include).
- Only `.televyignore` is read. `.gitignore`, `.ignore`, global gitignore, and parent directories outside the target root are not used.
- There are no built-in default excludes; only explicit rules in `.televyignore` take effect.
- Invalid rule lines are warned and ignored; other filesystem/scan errors keep existing failure behavior.
- Rule scope: backup scan + prepare quick stats. `settings import-bundle --compare-folder` is unchanged and does not apply `.televyignore`.
- Backup `run.finish` logs include ignore summary fields: `ignore_rule_files` and `ignore_invalid_rules`.

If upgrading from older versions that stored secrets in Keychain, run `televybackup secrets migrate-keychain`.

## Recovery key (TBK1)

To move restore capability across devices:

- Export (prints secret; requires explicit confirmation): `televybackup secrets export-master-key --i-understand`
- Import on a new device (reads from stdin): `televybackup secrets import-master-key`

## Config bundle (TBC2)

To move a whole working setup across devices (Settings v2 + required secrets), use the encrypted config bundle.
It is protected by a user-supplied passphrase (PIN/password).

- Export: set `TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE`, then run `televybackup --json settings export-bundle [--hint "<string>"]`
- Import (inspect only; reads from stdin): set `TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE`, then run `televybackup --json settings import-bundle --dry-run`
- Import (apply; reads JSON from stdin): set `TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE`, then run `televybackup --json settings import-bundle --apply`

Notes:

- The bundle is self-contained and includes `TBK1` (master key), but it is encrypted: importing a `TBC2:...` key requires the passphrase.
- The bundle includes an optional plaintext `hint` phrase (provided at export time) which is shown during import to help verify you're using the right bundle.
- MTProto session keys are not exported; they are regenerated on the new device as needed.

## Troubleshooting

If the macOS app shows **Recovery Key = Unavailable** or `Verify` fails with `daemon.unavailable` / `control.unavailable`:

- Ensure the daemon is running: `pgrep -x televybackupd` (the UI will also try to auto-start it).
- Ensure the UI/CLI/daemon use the same data dir:
  - Defaults: `~/Library/Application Support/TelevyBackup`
  - Overrides: `TELEVYBACKUP_CONFIG_DIR` / `TELEVYBACKUP_DATA_DIR`
- Check IPC sockets exist under the data dir:
  - `ipc/control.sock` (secrets presence / write actions)
  - `ipc/vault.sock` (vault/keychain ops)
- Check logs:
  - UI log: `TELEVYBACKUP_LOG_DIR/ui.log` (or `TELEVYBACKUP_DATA_DIR/logs/ui.log`)
  - Per-run logs (backup/restore/verify): `TELEVYBACKUP_DATA_DIR/logs/`

## Cross-device restore (latest)

After at least one successful backup, TelevyBackup updates a per-endpoint encrypted bootstrap catalog and pins it in the chat.
On a new device, you can restore without the old local SQLite:

- `televybackup restore latest --target-id <target_id> --target <path>`

Note: the pinned bootstrap catalog requires message pinning, so the endpoint chat should be a group/channel (or an `@username`), not a private 1:1 chat id.

## Cross-device incremental backup (remote-first index)

If you move to a new machine (or lose `index/index.sqlite`), TelevyBackup can continue incremental backups as long as:

- The pinned bootstrap catalog exists in the Telegram chat, and
- You imported the correct master key (`TBK1`) via `televybackup secrets import-master-key`.

By default, `televybackup backup run` enters a parallel `prepare` stage before `scan`:

- `index_sync`: if needed, download the remote latest index DB and atomically write `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`.
- `local_quick_stats`: metadata-only local walk to estimate source file count/bytes for progress denominator.
- `prepare` keeps running if local quick stats fail (progress may fall back to indeterminate), while `index_sync` keeps existing blocking semantics on hard errors (for example `bootstrap.decrypt_failed`).
- To force local-only behavior (offline/debug): `televybackup backup run --no-remote-index-sync`.

Backup progress semantics for UI/events:

- `prepare` phase is indeterminate only.
- `scan` / `scan_upload` / `upload` / `index` use a single determinate bar with monotonic layers:
  - `NeedUploadConfirmed <= UploadingCurrent <= BackedUp <= Scanned`
  - `Scanned`: source walk/read progress (`bytesRead` + file-count fallback).
  - `BackedUp`: source-level protected bytes, `(bytesUploadedSource + bytesDeduped) / sourceBytesTotal`.
  - `UploadingCurrent`: current payload upload progress in discovered upload workload.
  - `NeedUploadConfirmed`: confirmed uploaded payload progress in discovered upload workload.
- Need-upload metrics are phase-scoped:
  - `Need Upload (Disc.)` / `Remaining (Disc.)` during `scan` / `scan_upload`.
  - `Need Upload (Final)` / `Remaining (Final)` during `upload` / `index`.
- Snapshot filemap scans keep each multi-row statement bounded to 512 entries,
  match unchanged files against the base snapshot in one query per batch, and
  commit the scan transaction once. This keeps incremental scans compatible
  with base-chunk-copy while avoiding a SQLite commit and baseline lookup for
  every file.
- The temporary snapshot filemap is a single-writer WAL database: file metadata,
  file chunks, and newly read chunk rows use bounded multi-row writes; unchanged
  base chunks are seeded once and mapped in set-based batches. Full sync and an
  explicit WAL checkpoint run before the filemap is uploaded.
- `max_concurrent_uploads` bounds concurrent core document upload attempts across
  direct chunks, packs, index parts, and manifests. MTProto helper IPC runs off
  the async polling path, so the configured value is effective rather than
  merely a worker-pool size.

## Daemon (scheduled backups)

The scheduled runner is `televybackupd` (`crates/daemon/`). It uses the same `config.toml` and `secrets.enc` (vault key in Keychain).

The CLI can manage the local daemon explicitly:

```bash
televybackup daemon start
televybackup daemon status
televybackup daemon stop
televybackup daemon install-service
televybackup daemon service-status
televybackup daemon uninstall-service
```

`daemon start` returns after the local IPC service is ready. `daemon stop` cancels an active scheduled backup and waits up to 10 seconds for graceful shutdown. `install-service` explicitly installs the single product-managed per-user LaunchAgent; a different config/data directory requires `--replace`. Uninstall removes only managed service files and preserves user data. In the macOS app, quitting with schedules enabled offers a choice between quitting the app only and fully stopping the daemon; a full stop unloads the product-managed LaunchAgent (or the legacy Homebrew service) so its keep-alive setting cannot restart the process.

Release DMGs and native tool archives are built by the macOS package workflow. Verify `SHA256SUMS` before following the ad-hoc Gatekeeper instructions in [`packaging/INSTALL.md`](packaging/INSTALL.md). Homebrew templates under `packaging/homebrew/` are legacy compatibility artifacts and are not maintained by the release flow.

## Docs

- `docs/requirements.md`
- `docs/architecture.md`
- `docs/plan/README.md`
- `docs/quality-gates.md`

## VERSION-only release flow

The root `VERSION` file is the only product version source. It contains a stable `X.Y.Z` or RC `X.Y.Z-rc.N` value; Cargo package metadata is not a release fallback. The migration baseline is `0.9.2`, matching the existing `v0.9.2` identity.

Every PR targeting `main` must carry exactly one `type:*` label and one `channel:*` label. Patch plus stable prepares the next patch automatically. Major, minor, and every RC use a controlled exact VERSION dispatch. Docs and skip labels do not publish.

After source checks pass, the trusted preparation workflow adds only `VERSION` to the PR branch through GitHub-native `createCommitOnBranch` with `expectedHeadOid`; GitHub commit verification is required. Normal merge release consumes the merge SHA and committed VERSION. Manual recovery is limited to the same merge SHA and VERSION. See [`docs/specs/product-version-release-chain/SPEC.md`](docs/specs/product-version-release-chain/SPEC.md) and [`docs/quality-gates.md`](docs/quality-gates.md).
