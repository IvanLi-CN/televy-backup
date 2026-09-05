# Implementation

## Status

The distribution contract is implemented in the `th/feat/macos-release-distribution` fast-track. The first delivery covers packaging scripts, native CI, product-managed service commands, Settings state, and release workflow gates.

## Components

| Component | Location | Contract |
| --- | --- | --- |
| Build metadata | `crates/*/build.rs`, binary entrypoints | REQ-MRD-003 |
| macOS package assembly | `scripts/macos/package-release.sh`, `assemble-universal.sh`, `verify-release-assets.sh` | REQ-MRD-001, 002, 004 |
| Managed service | `crates/cli/src/service.rs` | REQ-MRD-005, 006 |
| GUI service control | `macos/TelevyBackupApp/SettingsWindow.swift` | REQ-MRD-007 |
| Release orchestration | `.github/workflows/release.yml`, `.github/workflows/release-completion.yml` | REQ-MRD-008; see `product-version-release-chain` for the current VERSION-only contract |

## Required Evidence

- `cargo fmt --all -- --check`
- Rust package and service contract tests
- `bash .github/scripts/test-release-scripts.sh`
- `bash scripts/macos/swift-unit-tests.sh`
- native macOS package matrix and Universal 2 verification in GitHub Actions
- shared testbox full-feature Rust validation

## Visual Evidence

The approved Settings service-state scenes are stored in the topic assets:
`service-installed`, `service-update`, `service-conflict`, and `service-failure`.
