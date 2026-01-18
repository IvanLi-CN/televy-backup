# TelevyBackup

macOS desktop backup app + Rust backend (work in progress).

## Prerequisites

- Rust (stable)
- Xcode (for macOS GUI)
- Bun

## Development

- Web: `bun run dev`
- Tauri: `bun run tauri:dev`

## Configuration

The GUI stores non-secret settings in `config.toml`, and secrets in macOS Keychain.

- `config.toml` location: `$APP_CONFIG_DIR/config.toml` (Tauri) or `TELEVYBACKUP_CONFIG_DIR/config.toml`
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
