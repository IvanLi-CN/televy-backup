# Implementation

## Status

The distribution contract is implemented in the `th/feat/macos-release-distribution` fast-track. The first delivery covers packaging scripts, native CI, product-managed service commands, Settings state, and release workflow gates.

## Components

| Component | Location | Contract |
| --- | --- | --- |
| Build metadata | `crates/*/build.rs`, binary entrypoints | REQ-MRD-003 |
| macOS package assembly | `scripts/macos/package-release.sh`, `assemble-universal.sh`, shared dmgbuild builder, `verify-dmg-layout.sh`, `verify-component-identity.sh`, `verify-release-assets.sh` | REQ-MRD-001, 002, 004, 010, 011 |
| Product brand bundle | `assets/brand/`, `scripts/macos/generate-app-icon-assets.sh`, `scripts/macos/generate-app-icon-previews.sh`, `scripts/macos/build-app.sh` | REQ-MRD-009 |
| Managed service | `crates/cli/src/service.rs` | REQ-MRD-005, 006 |
| Snapshot Access and mount services | `crates/snapshot-helper/`, `crates/cli/src/snapshot_service.rs`, `crates/cli/src/snapshot_mount_service.rs` | REQ-MRD-010 |
| GUI service control | `macos/TelevyBackupApp/SettingsWindow.swift` | REQ-MRD-007 |
| Release orchestration | `.github/workflows/release.yml`, `.github/workflows/release-completion.yml`, `.github/scripts/release_helper.py`, `.github/scripts/verify-macos-rc-acceptance.py` | REQ-MRD-008, REQ-MRD-010; see `product-version-release-chain` for the current VERSION-only contract |
| Homebrew GUI Cask | `Casks/televybackup.rb`, `scripts/homebrew/cask_release.py`, `.github/workflows/homebrew-cask.yml` | REQ-MRD-012 |

## Required Evidence

- `cargo fmt --all -- --check`
- Rust package and service contract tests
- `bash .github/scripts/test-release-scripts.sh`
- `bash scripts/macos/swift-unit-tests.sh`
- native macOS package matrix and Universal 2 verification in GitHub Actions
- `scripts/macos/verify-app-icon-assets.sh`, asset catalog `actool` compilation, and bundle `Info.plist`/resource inspection
- nested helper component lock, SHA-256/CDHash/designated-requirement comparison (including
  reconstruction of wrapped `codesign` requirement output, architecture-independent
  canonicalization of the `codesign` `# ` marker, non-quoted whitespace, commutative CDHash
  alternatives, and native-runner projection of the validated CDHash alternative segment),
  legacy artifact-digest compatibility, and RC artifact reuse inspection
- helper source resolution independent of the product RC ordinal; published Release manifest
  discovery and explicit no-source bootstrap mode are covered by resolver fixtures
- historical product checkouts use the trusted main policy SHA for Snapshot Access identity
  gates, so recovery repairs are applied without changing the recovered product source
- `snapshot-components.lock.json` bootstrap-tag preference for helper reuse across ordinary product
  versions, plus verified published prerelease `BUILD-MANIFEST.json` anchoring for RC2/stable reuse,
  including the final Universal bundle
- stable-gate download and verification of both RC Universal DMGs, checksums, tag-bound source
  commits, and helper identities before manual evidence is accepted
- real macOS RC1-to-RC2 migration check: one manual FDA grant after the old registration migration, then no FDA regrant for the ordinary main-app update
- stable publication approval through the `macos-release-acceptance` GitHub environment and the structured `TELEVYBACKUP_MACOS_RC_ACCEPTANCE_EVIDENCE` value, validated against the final manifest
- shared testbox full-feature Rust validation
- pinned `dmgbuild==1.6.7` settings, checked-in deterministic overlay/background bitmap digests (with the Swift generator retained for asset maintenance), semantic layout digest, hidden-resource allowlist, and native/Universal DMG parity
- final UDZO `hdiutil verify`, `diskutil verifyVolume`, plist attach, exact-device detach, and post-compression readback
- Homebrew Cask rendering and stable-release manifest/checksum fixtures; `brew audit --cask --strict`; downloaded Release DMG checksum, bundle-id, minimum-macOS, and Universal 2 checks
- built-in-token same-repository Cask PR and guarded squash merge using the published `SHA256SUMS` and `BUILD-MANIFEST.json`; no PAT, App, or repository secret
- controlled Finder acceptance on macOS 15 and the current supported macOS with scoped first-open screenshots

The controlled acceptance entrypoint is `scripts/macos/finder-dmg-acceptance.sh`. It is intentionally
manual-only, requires `TELEVYBACKUP_RUN_FINDER_ACCEPTANCE=1`, and writes Finder-window-scoped
evidence rather than running in release CI.

## Visual Evidence

The approved Settings service-state scenes are stored in the topic assets:
`service-installed`, `service-update`, `service-conflict`, and `service-failure`.
