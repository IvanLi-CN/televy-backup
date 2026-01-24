# TelevyBackup

macOS desktop backup app + Rust backend (work in progress).

## Prerequisites

- Rust (stable)
- Xcode Command Line Tools (for macOS GUI, `xcrun`)

## Development

- Build CLI/daemon: `cargo build`
- Build macOS app: `./scripts/macos/build-app.sh`
- Run macOS app: `./scripts/macos/run-app.sh`

## Configuration

The app stores non-secret settings in `config.toml`, and secrets in an encrypted local secrets store (`secrets.enc`).
macOS Keychain stores **only** the vault key used to decrypt `secrets.enc`.

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
