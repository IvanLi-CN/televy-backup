# ADR 0009: Minimal APFS Snapshot Mount Helper

## Decision

Keep Snapshot Access as a private embedded user-session FDA and file-reading helper inside
`TelevyBackup.app`, and add a
separate root `com.ivan.televybackup.snapshot-mount-helper` LaunchDaemon. Its only IPC methods are
`Status`, volume-scoped `StatusVolume`, `Reserve`, `Renew`, `RecordCleanup`, `Mount`, `Release`, and journal-bound `Cleanup`; `Reserve` claims the source volume before snapshot creation, binds the lease to the requested source path, proves that the peer UID can open that path, and records the pre-existing snapshot UUIDs with a validated-baseline marker. `RecordCleanup` invokes `tmutil localsnapshot` inside the root helper's serialized mutation section, requires exactly one new snapshot, and rechecks the inventory before and after journaling it; the Access process renews an in-flight reservation. Callers cannot submit a deletion manifest. A stale empty-manifest reservation may become a retained `released` tombstone after repeated baseline checks; every later `Reserve` audits that tombstone and moves it to `cleanup_pending` if the inventory changes, so a concurrent snapshot cannot lose its journal owner. Historical rows without a validated baseline remain blocked and cannot be used for deletion. `Release` revalidates the device/volume binding and requires the current inventory to equal the recorded baseline plus the journaled manifest before deletion; after deletion it requires the inventory to equal the original reservation baseline, then clears the recorded manifest and retains that baseline as the released tombstone. A different post-deletion inventory leaves the lease `cleanup_pending` with the original baseline. `Mount` validates the caller UID, revalidates the recorded source path access, and APFS
volume/device identity, creates a root-owned per-user lease directory that the caller cannot
replace, validates the UUID manifest before invoking `mount_apfs`, and passes the validated device
identity rather than the caller's source pathname; `Release` unmounts and deletes only the manifest
recorded for that lease, while `Cleanup` can delete only a manifest already recorded in a prepared
journal row. Legacy helper status is accepted only for the one-time migration probe; strict lease
acquisition rejects it before `Reserve`. Status classification is additive: recent rows, pending
cleanup, verified age-gated stale rows, and unknown rows remain distinct. An explicit reconcile
request uses the authenticated-UID-scoped Status and journal-bound Release methods after the
user-session Access service reports no active or pending lease; it includes disabled or removed
volume settings, never releases pending-cleanup or unknown rows, and never introduces arbitrary
lease or path cleanup. The current validated strict-mode baseline also
grants FDA to this helper's exact installed identity; that additional TCC authority does not expand
the helper into a file-reading or network boundary.

## Rationale

Testing on macOS showed that `tmutil localsnapshot` and FDA-controlled reads work as a regular user,
but `mount_apfs` fails with `Operation not permitted`. FDA is a TCC file-access decision and does
not grant mount(2) privilege. A root snapshot-and-mount boundary preserves the no-copy requirement without
making the backup daemon or file reader privileged.

## Consequences

Installing, updating, or removing the mount helper requires one administrator-authorized transaction.
Normal and scheduled backups do not authenticate. The helper stores a root-owned journal and can
recover mounts after a crash. It never reads user files, Keychain data, backup indexes, or network
data. Its existing system install path and version remain stable during an ordinary product app
update; this release performs compatibility checks only and never updates it automatically.
