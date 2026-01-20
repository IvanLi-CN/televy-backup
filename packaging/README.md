# Packaging

This folder contains Homebrew templates and build notes for the MVP.

## Homebrew (daemon)

- Formula: `packaging/homebrew/televybackupd.rb`
- Service: `brew services start televybackupd` (user-level LaunchAgent)

The service expects:

- `TELEVYBACKUP_CONFIG_DIR` (contains `config.toml`)
- `TELEVYBACKUP_DATA_DIR` (contains `index/index.sqlite`)

## Homebrew (GUI)

- Cask template: `packaging/homebrew/televybackup.rb`

The cask assumes a `.dmg` will be uploaded to GitHub Releases.

The GUI app is a native macOS `.app` bundle (SwiftUI/AppKit), built via `scripts/macos/build-app.sh`.
