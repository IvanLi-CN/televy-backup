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

To avoid conflicts with an installed/release build on the same machine, local development uses a separate app variant:

- **Release (stable) build**
  - Meaning: the build you install from a GitHub Release (DMG/ZIP) and place in `/Applications`.
  - Expected path: `/Applications/TelevyBackup.app`
  - Notes:
    - This is what "stable version" refers to in troubleshooting instructions and when an agent is asked to "restart stable".
    - If `/Applications/TelevyBackup.app` is not present, an agent must say so and ask you to install it (do not silently run a local build).

- **Prod app**
  - Name: `TelevyBackup`
  - Bundle ID: `com.ivan.televybackup`
  - Default vault key backend: **Keychain enabled**
  - Default local build output path: `target/macos-app/TelevyBackup.app` (this is NOT the release/stable build)
- **Dev app**
  - Name: `TelevyBackup Dev`
  - Bundle ID: `com.ivan.televybackup.dev`
  - Default vault key backend: **Keychain disabled** (override with `TELEVYBACKUP_DISABLE_KEYCHAIN=0`)
  - Default local build output path: `target/macos-app/TelevyBackup Dev.app`

Note: `scripts/macos/run-app.sh` will warn if you start the prod variant with `TELEVYBACKUP_DISABLE_KEYCHAIN=1`.

### How to confirm which app is running

The app name ("TelevyBackup") is not enough to disambiguate. Always confirm by **path**:

```bash
pgrep -fl 'TelevyBackup\\.app/Contents/MacOS/TelevyBackup' || true
```

You can also inspect an app bundle's bundle id / version:

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
- `secrets.enc` location: `TELEVYBACKUP_CONFIG_DIR/secrets.enc` (default: `~/Library/Application Support/TelevyBackup/secrets.enc`)
- Per-endpoint local index DB: `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`
  - Legacy (migration): `TELEVYBACKUP_DATA_DIR/index/index.sqlite` may exist but is ignored and auto-cleaned when all in-use per-endpoint DBs are usable.
- Per-run logs (NDJSON): `TELEVYBACKUP_LOG_DIR/` (override) or `TELEVYBACKUP_DATA_DIR/logs/` (default: `~/Library/Application Support/TelevyBackup/logs/`)
  - Log level filter: `TELEVYBACKUP_LOG` → `RUST_LOG` → default `debug`
- UI logs (macOS app): `TELEVYBACKUP_LOG_DIR/ui.log` (override) or `TELEVYBACKUP_DATA_DIR/logs/ui.log` (default: `~/Library/Application Support/TelevyBackup/logs/ui.log`)
- Keychain:
  - Vault key: key = `televybackup.vault_key` (Base64 32 bytes)
- Secrets store entries (inside `secrets.enc`):
  - Telegram bot token (used for MTProto bot sign-in): key = `[[telegram_endpoints]].bot_token_key` (per-endpoint)
  - Master key: key = `televybackup.master_key` (Base64 32 bytes)
  - MTProto API hash: key = `telegram.mtproto.api_hash` (default; key name configurable via `telegram.mtproto.api_hash_key`)
  - MTProto session: key = `[[telegram_endpoints]].mtproto.session_key` (per-endpoint; Base64)

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

By default, `televybackup backup run` performs a preflight `index_sync` step before `scan`:

- If needed, it downloads the remote latest index DB and atomically writes `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`.
- To force local-only behavior (offline/debug): `televybackup backup run --no-remote-index-sync`.

## Daemon (scheduled backups)

The scheduled runner is `televybackupd` (`crates/daemon/`). It uses the same `config.toml` and `secrets.enc` (vault key in Keychain).

Homebrew templates live under `packaging/homebrew/`.

## Docs

- `docs/requirements.md`
- `docs/architecture.md`
- `docs/plan/README.md`
