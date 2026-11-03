# ADR 0009: Minimal APFS Snapshot Mount Helper

## Decision

Keep Snapshot Access as a private embedded user-session FDA and file-reading helper inside
`TelevyBackup.app`, and add a
separate root `com.ivan.televybackup.snapshot-mount-helper` LaunchDaemon. Its IPC methods are
`Status`, volume-scoped `StatusVolume`, `Reserve`, `Renew`, `RecordCleanup`, `Mount`, `Release`, journal-bound `Cleanup`, and the explicit peer-UID-scoped `RepairLegacy`; `RepairLegacy` is the only method allowed to migrate a stale row without a validated baseline: it resolves the journaled volume UUID to the current APFS device, partitions current inventory by snapshot UUID, retains unrecorded snapshots as the new baseline, and requires an exact `statfs(2)` APFS source match before touching a mounted root. `Reserve` claims the source volume before snapshot creation, binds the lease to the requested source path, proves that the peer UID can open that path, and records the pre-existing snapshot UUIDs with a validated-baseline marker. `RecordCleanup` invokes `tmutil localsnapshot` inside the root helper's serialized mutation section, requires exactly one new snapshot, and rechecks the inventory before and after journaling it; the Access process renews an in-flight reservation. Callers cannot submit a deletion manifest. A stale empty-manifest reservation may become a retained `released` tombstone after repeated baseline checks; every later `Reserve` audits that tombstone and moves it to `cleanup_pending` if the inventory changes, so a concurrent snapshot cannot lose its journal owner. Unresolved identity, duplicate UUID, malformed manifest, changed inventory, and unrecorded snapshots remain blocked or retained as baseline and cannot be deleted. `Release` revalidates the device/volume binding and requires the current inventory to equal the recorded baseline plus the journaled manifest before deletion; after deletion it requires the inventory to equal the original reservation baseline, then clears the recorded manifest and retains that baseline as the released tombstone. A different post-deletion inventory leaves the lease `cleanup_pending` with the original baseline. `Mount` validates the caller UID, revalidates the recorded source path access, and APFS
volume/device identity, creates a root-owned per-user lease directory that the caller cannot
replace, validates the UUID manifest before invoking `mount_apfs`, and passes the validated device
identity rather than the caller's source pathname; `Release` unmounts and deletes only the manifest
recorded for that lease, while `Cleanup` can delete only a manifest already recorded in a prepared
journal row. Legacy helper status is accepted only for the one-time migration probe; strict lease
acquisition rejects it before `Reserve`. Status classification is additive: recent rows, pending
cleanup, verified age-gated stale rows, released inventory-drift rows, and unknown rows remain distinct. An explicit reconcile
stale-record recovery requires the current helper component and build identity, while a pending-cleanup retry may use the current or immediately previous cleanup-capable component only when the v4 wire identity and complete pending-only classification are verified. A separate inventory-reconcile operation may use the same v4 `Release` method for a verified released tombstone whose device/volume identity is unchanged but whose current inventory differs; it may update only that tombstone's baseline and never deletes snapshots or opens source files. Each request verifies the legacy aggregate
against the complete additive partition and candidate count before and after each release, and
uses the authenticated-UID-scoped Status and journal-bound Release methods after the user-session
Access service reports no active or pending lease; a current component with a stale or missing build
identity may use the same guarded replacement only when the complete pending-cleanup candidate
partition is verified and Snapshot Access is idle. A stale current-component build that reports the
live, pending, recoverable, and unknown partitions but omits only the newer inventory-drift field
may also use that guarded replacement after Snapshot Access proves idle; the replacement preserves
the journal and must classify it before readiness. It includes disabled or removed volume settings,
never releases pending-cleanup rows outside the explicit pending-only retry, never releases unknown
rows, and never introduces arbitrary lease or path cleanup.
The current validated strict-mode baseline also
grants FDA to this helper's exact installed identity; that additional TCC authority does not expand
the helper into a file-reading or network boundary.

If deletion finishes before a process crash, an exact match to the recorded reservation baseline
permits an idempotent released-tombstone transition. A mounted journal row without a positive
recorded mount-device identity remains unknown, and cleanup refuses to unmount it.

## Rationale

Historical terminal rows can retain a caller-owned data-directory mount layout that cannot
enter root-owned directory cleanup. Explicit repair can retire only the journal identity of
stale owner-matching `released` rows with zero mount device, after all recorded snapshot UUIDs
are absent and a complete kernel mount table shows no matching source or mount at/below the
no-follow-validated historical root. Repeated identity and inventory checks precede a guarded
update that rejects another non-released owner on the volume. Unrecorded inventory remains the
baseline. No operation touches the historical directory or deletes snapshots; unresolved rows
continue to block strict mode.

Testing on macOS showed that `tmutil localsnapshot` and FDA-controlled reads work as a regular user,
but `mount_apfs` fails with `Operation not permitted`. FDA is a TCC file-access decision and does
not grant mount(2) privilege. A root snapshot-and-mount boundary preserves the no-copy requirement without
making the backup daemon or file reader privileged.

A mount denied before device capture is distinct from a mounted filesystem with unknown identity.
The former can reuse journal-bound UUID deletion after a complete kernel absence proof, repeated
volume/root checks, and exact pre/post inventory checks. It must be retryable through `Release`
after authorization is corrected. Treating device zero as unconditional failure otherwise traps a
new snapshot and makes every prior tombstone appear drifted. A still-mounted or uncertain root
remains blocked; there is no permission or deletion boundary expansion.

## Consequences

Installing, updating, or removing the mount helper requires one administrator-authorized transaction.
Normal and scheduled backups do not authenticate. The helper stores a root-owned journal and can
recover mounts after a crash. It never reads user files, Keychain data, backup indexes, or network
data. Its existing system install path and version remain stable during an ordinary product app
update; this release performs compatibility checks only and never updates it automatically.

Backup acquisition is an explicit operation that may reuse the inventory-only Release reconciliation while holding the daemon maintenance lock. It requires an idle Access service and a complete released-only drift partition. This does not authorize deletion, recovery of unknown records, or mutation on status refresh. Mounted filesystem-root permissions are source metadata; cleanup pins the recorded device and protected managed ancestors instead of requiring the mounted root to retain empty-directory permissions.
