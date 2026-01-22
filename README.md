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

- `config.toml` location: `TELEVYBACKUP_CONFIG_DIR/config.toml` (default: `~/Library/Application Support/TelevyBackup/config.toml`)
- `secrets.enc` location: `TELEVYBACKUP_CONFIG_DIR/secrets.enc` (default: `~/Library/Application Support/TelevyBackup/secrets.enc`)
- Local index DB: `$APP_DATA_DIR/index/index.sqlite` or `TELEVYBACKUP_DATA_DIR/index/index.sqlite`
- Per-run logs (NDJSON): `TELEVYBACKUP_LOG_DIR/` (override) or `TELEVYBACKUP_DATA_DIR/logs/` (default: `~/Library/Application Support/TelevyBackup/logs/`)
  - Log level filter: `TELEVYBACKUP_LOG` → `RUST_LOG` → default `debug`
- Keychain:
  - Vault key: key = `televybackup.vault_key` (Base64 32 bytes)
- Secrets store entries (inside `secrets.enc`):
  - Telegram bot token (used for MTProto bot sign-in): key = `telegram.bot_token` (default)
  - Master key: key = `televybackup.master_key` (Base64 32 bytes)
  - MTProto API hash: key = `telegram.mtproto.api_hash` (default)
  - MTProto session: key = `telegram.mtproto.session` (Base64)

If upgrading from older versions that stored secrets in Keychain, run `televybackup secrets migrate-keychain`.

## Daemon (scheduled backups)

The scheduled runner is `televybackupd` (`crates/daemon/`). It uses the same `config.toml` and `secrets.enc` (vault key in Keychain).

Homebrew templates live under `packaging/homebrew/`.

## Docs

- `docs/requirements.md`
- `docs/architecture.md`
- `docs/plan/README.md`
