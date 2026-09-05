# Implementation

## Components

| Component | Location |
| --- | --- |
| Version resolver and grammar | `VERSION`, `scripts/product-version.py` |
| Rust build identity | `crates/cli/build.rs`, `crates/daemon/build.rs`, `crates/mtproto-helper/build.rs` |
| macOS identity and assets | `scripts/macos/build-app.sh`, `package-release.sh`, `assemble-universal.sh`, `generate-release-manifest.sh`, `verify-release-assets.sh` |
| Label and chain validation | `.github/scripts/label-gate.sh`, `release_chain.py`, `release_preparation.py`, `release_completion.py` |
| Trusted preparation | `.github/workflows/release-preparation.yml` |
| Completion and release | `.github/workflows/release-completion.yml`, `.github/workflows/release.yml` |
| Quality declaration | `.github/quality-gates.json`, `.github/release-contract.json` |

## Migration

The initial root `VERSION` value is `0.9.2`, matching the existing published `v0.9.2` identity. The completion check has a one-time migration path for a PR that adds exactly this value while the base has no VERSION. It reports migration and cannot trigger a product release.

## Verification

Local verification runs the resolver unit tests, release-chain fixtures, package manifest fixture, shell syntax checks, YAML parsing, Rust checks, and the quality-gates checker. macOS native package and Swift matrix jobs remain the authoritative hosted checks.
