# APFS Snapshot Consistency

> Canonical topic for filesystem-timepoint backup reads on macOS APFS volumes.

## Context and Scope

TelevyBackup must read every file in a strict backup from one filesystem timepoint while live
files continue changing. This topic owns the user-session Snapshot Access app, its IPC contract,
the brokered core source, per-volume preferences, lifecycle cleanup, and macOS packaging. It does
not change application-level transaction semantics, Time Machine configuration, or remote storage
formats.

## Goals and Non-goals

### Goals

- Provide a separate, windowless Snapshot Access process embedded inside the single visible
  `TelevyBackup.app` at `Contents/Library/LoginItems/TelevyBackup Snapshot Access.app`, plus a
  narrowly-scoped root mount helper. The current validated strict-mode baseline grants FDA to each
  exact installed identity; the helper additionally needs root because macOS mount(2) privilege is
  not granted by FDA.
- Address sources by configured target ID and APFS Volume UUID, never by an arbitrary IPC path.
- Stream directory metadata and file bytes through an opaque lease without exposing the snapshot
  mount path to the daemon.
- Fail closed for strict volumes when FDA, snapshot creation, ownership, mounting, reading, or
  cleanup is unavailable.
- Keep CLI-only scheduled operation available through a per-user LaunchAgent with no administrator
  prompt after the one-time mount-helper installation.

### Non-goals

- No root backup daemon, root file reader, `fs_snapshot_*` entitlement, or staging copy. A minimal
  root LaunchDaemon is limited to mount, unmount, and UUID-scoped snapshot cleanup.
- No activity-directory fallback in strict mode and no changes to Time Machine policy.
- No defense against a malicious process running as the same macOS user.

## Requirements

### REQ-APFS-001: Access process boundary

The package MUST include exactly one top-level `TelevyBackup.app`. It MUST contain a separate
Snapshot Access app at `Contents/Library/LoginItems/TelevyBackup Snapshot Access.app` with bundle
identifier `com.ivan.televybackup.snapshot-access` and `LSUIElement=true`. It MUST run as the logged-in user,
reject root execution, keep its journal/socket/mount directories private to that user, and never
read Keychain data, encrypt backup data, or upload files.

### REQ-APFS-002: Versioned restricted IPC

The Access app MUST expose versioned `Status`, `ProbeVolume`, `AcquireLease`, `ScanPage`,
`OpenReadStream`, `ReadStream`, `CloseReadStream`, and `ReleaseLease` methods. Requests MUST use a
configured target ID or an opaque lease/stream ID. Peer UID, canonical source, APFS Volume UUID,
free-space threshold, nested mounts, and relative paths MUST be checked before access.

### REQ-APFS-003: Snapshot ownership and cleanup

`AcquireLease` MUST snapshot the target volume, uniquely confirm the snapshot created by that
  invocation, record its UUID/device manifest before serving data, and ask the root mount helper to
  mount it below a private app directory. Release and crash recovery MUST unmount and delete only
  manifest-recorded snapshots; ambiguous ownership or failed cleanup MUST block the volume's next
  strict backup.

The mount helper MUST run as a root LaunchDaemon with FDA manually granted to its exact installed
identity, expose a separate peer-UID-checked Unix socket, and accept only `Status`, `Mount`,
`Release`, and UUID-scoped `Cleanup`. It MUST NOT read source files, configuration, Keychain
material, backup indexes, or network data. Its root-owned journal records lease, mount root,
volume/device identity, and the exact snapshot UUID manifest.

### REQ-APFS-004: Brokered strict reads

The core MUST retain the configured logical source path in indexes and manifests while obtaining
all strict-mode entries and file bytes from the brokered source. Symlinks MUST NOT be followed,
nested mounted volumes MUST fail the scan, and a broker read error MUST fail the backup without
publishing a Backup Snapshot. The lease MUST be released once source bytes have been read into the
encrypted upload queue.

### REQ-APFS-005: Per-volume configuration and scheduling

`config.toml` MUST persist `snapshot_volumes.<APFS Volume UUID>.enabled`. Verification MUST
discover the current volume UUID and explain unsupported filesystems, insufficient space, missing
sources, or FDA failures. Targets on the same volume MUST serialize snapshot leases; a manual run
MUST supersede one pending scheduled batch, and cleanup-pending state MUST prevent a new strict
lease.

### REQ-APFS-006: User installation and observability

The main app MUST register the embedded agent with `SMAppService.agent` and its plist MUST use a
bundle-relative `BundleProgram`. The CLI MUST expose only read-only `snapshot-access status` plus
internal migration operations; it MUST reject arbitrary external app paths and MUST NOT create a
new user LaunchAgent plist. Settings MUST show the exact embedded Access app and mount helper paths,
component versions, migration state, service reachability, active leases, pending cleanup, and a
link to System Settings. It MUST identify both paths as FDA requirements for strict mode, report
only observable FDA evidence, and never present service reachability as proof of a helper TCC grant.
The first layout migration is user-level; mount helper install, update, and uninstall remain
explicit administrator-authorized transactions and are the only privileged setup operation.
Migration commit and rollback MUST require the transaction id and a short-lived owner token from
the same prepare operation; missing or mismatched transaction credentials MUST fail closed.

## Compatibility

The Access app is started by the main app's per-user `SMAppService` LaunchAgent and can be used
without the GUI after registration. The v0.9.8 helper remains readable for the one-time lease
probe, while only the current component may be committed or used for strict reads. RC1 establishes
the embedded helper identity; ordinary future product-version RC1 builds reuse the approved
artifact named by the component lock, and RC2/stable reuse that version's exact Universal artifact,
so ordinary main-app updates do not request FDA again. A real Access helper code or FDA behavior
change creates a new component version and requires explicit migration and manual FDA review.
Production migration MUST run from `/Applications/TelevyBackup.app`; direct DMG launches MUST fail
closed instead of registering a mounted helper. Existing settings without `snapshot_volumes` remain
valid and default to live mode until a volume is verified and enabled.

## Verification

### VER-APFS-001: Contract and package

Covers: REQ-APFS-001, REQ-APFS-002.
Rust/Swift tests, the spec contract checker, and package asset verification prove that target IDs,
volume UUID preferences, additive status fields, the separate app bundle, the minimal mount-helper
artifact, and user/system service paths are present without a privileged backup reader.

### VER-APFS-002: Access lifecycle and boundary

Covers: REQ-APFS-003.
Snapshot Access and mount-helper unit tests prove fixed command usage, peer UID enforcement, arbitrary-path
rejection, private journal/mount ownership, unique snapshot ownership, UUID-only cleanup, and
foreign journal recovery isolation.

### VER-APFS-003: Brokered data integrity

Covers: REQ-APFS-004.
Core and daemon tests prove metadata and bytes come from the broker, post-snapshot live mutations
are absent, logical source identity is preserved, symlinks are not followed, and broker read
failures publish no strict Backup Snapshot.

### VER-APFS-004: macOS integration

Covers: REQ-APFS-005, REQ-APFS-006.
On a controlled APFS fixture, the workspace-built non-root Access app creates the snapshot and the
root mount helper performs only the mount operation; the daemon receives no mount path, configured
source roots can be sampled, and cleanup uses only the recorded manifest. FDA changes are performed
manually by the owner and are not automated by tests.

### VER-APFS-005: RC migration and authorization continuity

On the controlled macOS 15 fixture, migrate the v0.9.8 external registration to RC1, grant FDA
once to the embedded Snapshot Access app in System Settings, and prove a strict backup can read a
protected source. Replace `/Applications/TelevyBackup.app` with RC2 from the same RC1 Universal
helper artifact, compare the helper SHA-256/CDHash/designated requirement and repeat the strict
backup without granting FDA again. Record that the root mount helper remains at its installed path
and version. The result is a manual release artifact; tests MUST NOT modify TCC state.

## Related ADRs

- [0008-apfs-snapshot-access-app](../../adr/0008-apfs-snapshot-access-app.md)
- [0009-apfs-snapshot-mount-helper](../../adr/0009-apfs-snapshot-mount-helper.md)
- [0010-identity-stable-single-product-release](../../adr/0010-identity-stable-single-product-release.md)

## Visual Evidence

The Settings Snapshots screen is captured from the deterministic `SettingsUIDemo` source:

- [Snapshot Access ready](assets/snapshot-access-ready.png)
- [Snapshot Access requires FDA](assets/snapshot-access-fda-required.png)
