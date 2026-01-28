# TelevyBackup

macOS desktop backup app + Rust backend (work in progress).

## Prerequisites

- Rust (stable)
- Xcode Command Line Tools (for macOS GUI, `xcrun`)

## Development

- Build CLI/daemon: `cargo build`
- Build macOS app: `./scripts/macos/build-app.sh`
- Run macOS app: `./scripts/macos/run-app.sh`

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

Set `TELEVYBACKUP_DISABLE_KEYCHAIN=1` when starting the daemon. In this mode, the daemon will **not** access Keychain
and will use a local vault key file instead:

- Default: `TELEVYBACKUP_CONFIG_DIR/vault.key` (default config dir: `~/Library/Application Support/TelevyBackup/`)
- Override: `TELEVYBACKUP_VAULT_KEY_FILE=<path>`

Example:

```bash
TELEVYBACKUP_DISABLE_KEYCHAIN=1 televybackupd
```

Important: `vault.key` on disk is a **security downgrade**. Treat it like a secret and only use this mode for local dev.

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
- Local index DB: `$APP_DATA_DIR/index/index.sqlite` or `TELEVYBACKUP_DATA_DIR/index/index.sqlite`
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

## Cross-device restore (latest)

After at least one successful backup, TelevyBackup updates a per-endpoint encrypted bootstrap catalog and pins it in the chat.
On a new device, you can restore without the old local SQLite:

- `televybackup restore latest --target-id <target_id> --target <path>`

## Daemon (scheduled backups)

The scheduled runner is `televybackupd` (`crates/daemon/`). It uses the same `config.toml` and `secrets.enc` (vault key in Keychain).

Homebrew templates live under `packaging/homebrew/`.

## Docs

- `docs/requirements.md`
- `docs/architecture.md`
- `docs/plan/README.md`
