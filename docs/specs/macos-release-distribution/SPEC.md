# macOS Release Distribution and Product-Managed Daemon

> Current packaging and service requirements remain here. Release orchestration is governed by [`product-version-release-chain`](../product-version-release-chain/SPEC.md); implementation coverage is tracked in `IMPLEMENTATION.md` and lifecycle history in `HISTORY.md`.

## Context and Scope

This topic owns the macOS distribution surface for TelevyBackup: three GUI DMGs, native arm64 and x86_64 tool archives, Universal 2 binaries, checksums, build manifests, and the product-managed per-user daemon service.

It does not own backup formats, Telegram protocol behavior, Apple Developer ID signing, notarization, App Store delivery, automatic updates, or Homebrew formula maintenance.

## Terms

- **Release version**: the complete semver-like version shown to users, including an RC suffix.
- **Build number**: a deterministic numeric `CFBundleVersion` derived from the source history.
- **Managed service**: the single user LaunchAgent labeled `com.ivan.televybackup.daemon`.
- **Environment**: the exact config and data directory pair passed to the daemon.
- **Universal 2**: a Mach-O binary containing both arm64 and x86_64 slices.
- **Brand bundle**: the compiled `Assets.car` App Icon catalog, the `TelevyBackup.icns` compatibility fallback, and the three runtime SVGs under `Contents/Resources/Brand`.

## Requirements

### REQ-MRD-001: Traceable release assets

Every stable or RC release MUST publish `TelevyBackup-<version>.dmg`, `TelevyBackup-<version>-arm64.dmg`, `TelevyBackup-<version>-x86_64.dmg`, `televybackup-tools-<version>-arm64.tar.gz`, `televybackup-tools-<version>-x86_64.tar.gz`, `SHA256SUMS`, and `BUILD-MANIFEST.json` only after all asset checks pass. Each DMG MUST contain one installable app entry named `TelevyBackup.app`; version and architecture belong in the downloadable DMG filename, not the app entry name.

### REQ-MRD-002: Native build matrix

arm64 assets MUST be built on `macos-15`; x86_64 assets MUST be built on `macos-15-intel`. Universal 2 assembly MUST combine those native slices and verify all four embedded executables.

### REQ-MRD-003: Version observability

The App MUST use a numeric `CFBundleShortVersionString`, deterministic numeric `CFBundleVersion`, and a full `TelevyBackupReleaseVersion` key. `televybackup`, `televybackupd`, and `televybackup-mtproto-helper` MUST expose `--version` with release version and source commit.

### REQ-MRD-004: Controlled signing

Release assets MUST use strict ad-hoc signatures. The build MUST NOT read Apple certificates, notarization credentials, or silently remove quarantine. Installation guidance MUST require SHA-256 verification before a user performs any Gatekeeper action.

### REQ-MRD-005: Product-managed service

The CLI MUST provide `daemon install-service [--replace]`, `daemon uninstall-service`, and `daemon service-status`. Installation is explicit, idempotent in the same environment, and fail-closed for a different environment unless `--replace` is supplied. Only one managed service exists per user.

### REQ-MRD-006: Transactional ownership

Managed binaries live under a versioned directory in the user's TelevyBackup Application Support tree. Install MUST stage and atomically activate a new version, retaining at most active and previous managed versions. Failure MUST restore the prior plist and binary pointer. Uninstall MUST remove only managed service files and preserve config, data, logs, indexes, and keychain entries.

### REQ-MRD-007: GUI lifecycle

The Settings Schedule page MUST default the service switch to off, display installed/update/conflict/failure states, prevent duplicate operations, and invoke the same CLI service contract. Daemon business operations continue to use the authenticated control IPC boundary.

### REQ-MRD-008: Release atomicity and backfill

Normal release runs MUST build and validate all assets before creating a draft Release and MUST make it public only after upload and hash verification. Exact-tag backfill MUST default to build-only, use the tag's source commit without moving the tag or bumping its version, and reject an existing asset name with a different hash.

### REQ-MRD-009: Product brand assets

Every GUI app bundle MUST contain `TelevyBackup.icns`, declare it through
`CFBundleIconFile`, declare `AppIcon` through `CFBundleIconName`, and include
`Assets.car` compiled from `assets/brand/macos/Assets.xcassets`. It MUST also
include the light UI, dark UI, and monochrome template SVGs under
`Contents/Resources/Brand`. The iconset, asset catalog, and runtime SVGs MUST be
generated from the selected Graphite Azure geometry without embedded raster data.

## Compatibility

The `v0.9.0` backfill is built from source commit `0f283ce8ccbc30c56728c1d6c0366b76d8972772` and may only add packaging metadata and a manual LaunchAgent template. Full service commands, GUI switch, and version interfaces begin with the next patch release. Existing Homebrew services remain detectable for a migration warning but are not maintained by this topic.

## Verification

### VER-MRD-001

Covers: REQ-MRD-001, REQ-MRD-008. Release script contract tests and the package workflow's draft/upload/hash gate provide the evidence.

### VER-MRD-002

Covers: REQ-MRD-002, REQ-MRD-004. The native package matrix, asset verifier, and ad-hoc codesign inspection provide the evidence.

### VER-MRD-003

Covers: REQ-MRD-003. Binary version contract tests and Info.plist inspection provide the evidence.

### VER-MRD-004

Covers: REQ-MRD-005, REQ-MRD-006. CLI service tests and the transaction fixture provide the evidence.

### VER-MRD-005

Covers: REQ-MRD-007. Swift unit tests and isolated Settings snapshots provide the evidence.

## Verification Map

| Requirement | Verification |
| --- | --- |
| REQ-MRD-001, 008 | release script contract tests; package workflow |
| REQ-MRD-002, 004 | native package matrix; asset verifier; codesign inspection |
| REQ-MRD-003 | binary version contract tests; plist inspection |
| REQ-MRD-005, 006 | CLI service tests; transaction fixture |
| REQ-MRD-007 | Swift unit tests; isolated Settings snapshots |
| REQ-MRD-009 | app build; brand and App Icon asset verifiers; bundle inspection |

## Related ADRs

- [0002-settings-window-ipc-only](../../adr/0002-settings-window-ipc-only.md)
- [0003-product-managed-daemon-launchagent](../../adr/0003-product-managed-daemon-launchagent.md)

## Visual Evidence

Approved isolated Dev Settings window scenes for the managed background service:

- [service-installed](assets/service-installed.png)
- [service-update](assets/service-update.png)
- [service-conflict](assets/service-conflict.png)
- [service-failure](assets/service-failure.png)

Capture contract: `target_program=com.ivan.televybackup.dev`, Settings window only. The
images contain no desktop or unrelated windows.

Brand bundle evidence is covered by the asset verifier and the Popover/menu-bar
visual evidence in the related UI Specs.

App Icon review references are stored under
`assets/brand/macos/previews/`: Default, Dark, and Mono/Tinted appearances are
shown at 48px, 128px, and 512px with rounded, squircle, and circle masks.
