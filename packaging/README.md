# Packaging

This folder contains the macOS release installation guide and legacy Homebrew templates.

## Release packages

Native packaging is driven by `scripts/macos/package-release.sh`, `assemble-universal.sh`, and
`verify-release-assets.sh`. The release workflow publishes the three DMGs, two native tool
archives, `SHA256SUMS`, and `BUILD-MANIFEST.json` only after the full asset gate passes.

## Homebrew (daemon)

- Formula: `packaging/homebrew/televybackupd.rb`
- Service: `brew services start televybackupd` (user-level LaunchAgent)

The macOS app's **Quit Completely** action unloads this LaunchAgent after requesting a graceful daemon shutdown. Restart it explicitly with `brew services start televybackupd` or `televybackup daemon start`.

The service expects:

- `TELEVYBACKUP_CONFIG_DIR` (contains `config.toml`)
- `TELEVYBACKUP_DATA_DIR` (contains `index/index.sqlite`)

## Homebrew (legacy)

- Cask template: `packaging/homebrew/televybackup.rb`

Homebrew formulas are retained for existing users but are not maintained by the product release
flow. New installs should use the signed release DMG or tool archive and the product-managed
LaunchAgent (`televybackup daemon install-service`). The cask's historical URL and version are not
the current release contract.

The GUI app is a native macOS `.app` bundle (SwiftUI/AppKit), built via `scripts/macos/build-app.sh`.
