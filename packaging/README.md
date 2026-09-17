# Packaging

This folder contains the macOS release installation guide and the legacy Homebrew daemon formula.

## Release packages

Native packaging is driven by `scripts/macos/package-release.sh`, `assemble-universal.sh`, and
`verify-release-assets.sh`. The release workflow publishes the three DMGs, two native tool
archives, `SHA256SUMS`, and `BUILD-MANIFEST.json` only after the full asset gate passes.

## Homebrew GUI app

The GUI Cask lives at the root tap path [`Casks/televybackup.rb`](../Casks/televybackup.rb), outside `packaging/`. Add this repository as the tap with its explicit URL, then install the fully-qualified Cask:

```sh
brew tap IvanLi-CN/televy-backup https://github.com/IvanLi-CN/televy-backup.git
brew install --cask IvanLi-CN/televy-backup/televybackup
```

The Cask follows stable releases only, installs the app bundle, and leaves Gatekeeper quarantine intact. First launch may require manual Gatekeeper approval because the release is ad-hoc signed and not notarized.

### Updating the Cask

After a stable Release is published, the same-repository workflow updates the Cask from immutable
release metadata, opens a PR, dispatches its checks, and merges it after the exact head passes. It
uses only GitHub's built-in `GITHUB_TOKEN`; no PAT, App, or repository secret is required. The
workflow can be retried manually for an already-published stable tag.

## Homebrew daemon Formula (legacy)

- Formula: `packaging/homebrew/televybackupd.rb`
- Service: `brew services start televybackupd` (user-level LaunchAgent)

The macOS app's **Quit Completely** action unloads this LaunchAgent after requesting a graceful daemon shutdown. Restart it explicitly with `brew services start televybackupd` or `televybackup daemon start`.

The service expects:

- `TELEVYBACKUP_CONFIG_DIR` (contains `config.toml`)
- `TELEVYBACKUP_DATA_DIR` (contains `index/index.sqlite`)

## Homebrew daemon compatibility

The daemon Formula is retained for existing users but is not maintained by the product release
flow. New daemon installs should use the published release DMG or tool archive and the product-managed
LaunchAgent (`televybackup daemon install-service`).

The GUI app is a native macOS `.app` bundle (SwiftUI/AppKit), built via `scripts/macos/build-app.sh`.
