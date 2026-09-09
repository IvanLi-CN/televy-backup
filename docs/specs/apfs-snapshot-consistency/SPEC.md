# APFS Snapshot Consistency

## Status

Active.

## Related ADRs

- `docs/adr/0007-apfs-snapshot-access.md`
- `docs/adr/0008-apfs-snapshot-mount-helper.md`

## Contract

When a target is configured for strict mode, every byte and metadata value used
by a backup must come from one APFS snapshot of the target's volume. A snapshot
acquisition, mount, scan, read, or cleanup failure fails the run and never falls
back to the live directory.

The logical source path remains the target's configured path. The physical
snapshot mount path is private to Snapshot Access and is never returned to the
daemon, CLI, GUI, manifest, history, or remote storage.

## Components

- `TelevyBackup Snapshot Access.app` is a user-level, `LSUIElement` process with
  FDA. It owns snapshot acquisition, target validation, snapshot enumeration,
  directory pages, bounded file streams, and lease state.
- The root mount helper is a separate minimal `LaunchDaemon` with FDA. It only
  mounts and unmounts a recorded snapshot and performs UUID-exact cleanup. It
  cannot access vaults, Keychain, backup data, or network storage.
- The existing GUI, CLI, and daemon remain non-root and do not receive a
  snapshot mount path.

## Configuration

`SettingsV2` keeps its schema version and adds a default-empty
`snapshot_volumes` map keyed by APFS Volume UUID. Each value contains only the
strict-mode preference. A UUID that is absent, moved, or no longer matches the
configured target remains disabled after import.

## IPC

Snapshot Access exposes a versioned private Unix IPC protocol with these
operations: `Status`, `ProbeVolume`, `AcquireLease`, `ScanPage`,
`OpenReadStream`, `ReadStream`, `CloseReadStream`, `ReleaseLease`, and
`VerifyTimepoint`.

Requests use configured target IDs, never arbitrary paths. The daemon coalesces
same-volume work before admission and each active lease is bound to one
configured target and its validated volume; a second lease for that volume is
rejected until cleanup completes.
The stream protocol uses bounded frames and backpressure; no response exposes a
mount point or snapshot UUID.

## Scheduling and failure

There is at most one active and one pending batch per volume. A manual request
supersedes a pending scheduled request. A timeout, cancellation, crash recovery
failure, or cleanup-pending state blocks the next strict run for that volume.
The caps are 10 minutes for snapshot creation, 30 seconds for mounting, and 2
minutes for release and cleanup.

## Verification and privacy

Enabling strict mode requires an explicit full-chain verification that creates a
random probe inside the configured target, captures a snapshot, mutates the live
probe, reads the old bytes through Snapshot Access, and removes the probe and
snapshot. The shipped CLI and Settings UI use this same operation. Sanitized
output contains only boolean assertions and generic error codes.

## Compatibility

The application supports macOS 15 and APFS. No `fs_snapshot_*` entitlement,
whole-volume staging copy, Time Machine configuration change, or application-
transaction consistency guarantee is part of this contract.
