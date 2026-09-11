# ADR 0009: Minimal APFS Snapshot Mount Helper

## Decision

Keep Snapshot Access as a private embedded user-session FDA and file-reading helper inside
`TelevyBackup.app`, and add a
separate root `com.ivan.televybackup.snapshot-mount-helper` LaunchDaemon. Its only IPC methods are
`Status`, `Mount`, `Release`, and UUID-scoped `Cleanup`; `Mount` validates the caller UID, APFS volume/device identity,
private mount directory, and UUID manifest before invoking `mount_apfs`; `Release` unmounts and
deletes only the manifest recorded for that lease. The current validated strict-mode baseline also
grants FDA to this helper's exact installed identity; that additional TCC authority does not expand
the helper beyond its mount-only protocol.

## Rationale

Testing on macOS showed that `tmutil localsnapshot` and FDA-controlled reads work as a regular user,
but `mount_apfs` fails with `Operation not permitted`. FDA is a TCC file-access decision and does
not grant mount(2) privilege. A root mount-only boundary preserves the no-copy requirement without
making the backup daemon or file reader privileged.

## Consequences

Installing, updating, or removing the mount helper requires one administrator-authorized transaction.
Normal and scheduled backups do not authenticate. The helper stores a root-owned journal and can
recover mounts after a crash. It never reads user files, Keychain data, backup indexes, or network
data. Its existing system install path and version remain stable during an ordinary product app
update; this release performs compatibility checks only and never updates it automatically.
