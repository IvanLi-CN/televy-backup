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

The app stores non-secret settings in `config.toml`, and secrets in macOS Keychain.

- `config.toml` location: `TELEVYBACKUP_CONFIG_DIR/config.toml` (default: `~/Library/Application Support/TelevyBackup/config.toml`)
- Local index DB: `$APP_DATA_DIR/index/index.sqlite` or `TELEVYBACKUP_DATA_DIR/index/index.sqlite`
- Keychain secrets:
  - Telegram bot token: key = `telegram.bot_token` (default)
  - Master key: key = `televybackup.master_key` (Base64 32 bytes)

## Daemon (scheduled backups)

The scheduled runner is `televybackupd` (`crates/daemon/`). It uses the same `config.toml` and Keychain secrets.

Homebrew templates live under `packaging/homebrew/`.

## Docs

- `docs/requirements.md`
- `docs/architecture.md`
- `docs/plan/README.md`
