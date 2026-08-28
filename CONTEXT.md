# TelevyBackup

TelevyBackup protects local data in remote storage and restores protected data back to a local destination. Its macOS client presents the current transfer activity in the menu bar.

## Transfer Activity

**Backup**:
A task that transfers protected local backup data to remote storage. It is the upload direction of a transfer activity.
_Avoid_: Upload

**Restore**:
A task that transfers protected backup data from remote storage to a local destination. It is the download direction of a transfer activity.
_Avoid_: Download

**Menu Bar Activity State**:
A global macOS presentation derived from all current tasks rather than from one target. Its states are Idle, Error, Backing Up, Restoring, Verifying, and Bidirectional Sync.
_Avoid_: Per-target status

**Transfer Direction**:
The upload or download data-flow declared by an active task, including periods when its current transfer rate is zero.
_Avoid_: Current network rate

**Menu Bar Rate Slot**:
A fixed-width, four-character field for one visible Transfer Direction in the macOS menu bar. It is rendered in a monospaced font, is separate from the arrow and `/s` suffix, and prevents instantaneous rate formatting from changing the status item's measured width.
_Avoid_: Detailed transfer rate

**Bidirectional Sync**:
A Menu Bar Activity State in which upload and download activities coexist, or a native sync activity declares both directions.
_Avoid_: Two-way backup

**Verification**:
A task that reads protected remote data to validate it without restoring that data to a local destination.
_Avoid_: Restore

**Current Task Failure**:
A failure emitted by a backup, restore, or verification task in the current live session. Historical run results do not determine the Menu Bar Activity State.

TelevyBackup also preserves source directories as independently addressable backup snapshots. The history experience distinguishes the execution of a backup from the snapshot it successfully produces.

## Backup History

**Backup Run**:
One attempted execution of a backup task, including attempts that fail or are cancelled. A run may not produce a snapshot.
_Avoid_: Backup, snapshot

**Backup Snapshot**:
The completed, restorable file-tree state produced by a successful backup run.
_Avoid_: Backup run, backup task

**Snapshot Retention Window**:
The most recent backup snapshots of a target that remain available for detailed inspection and restore.
_Avoid_: Backup history, log retention

**Baseline Snapshot**:
The directly preceding successful snapshot of the same target that a backup snapshot is compared with.
_Avoid_: Previous run, latest snapshot

**File Tree Change**:
A difference between a file entry in a backup snapshot and its baseline: added, deleted, or changed. A regular file is changed when its type, size, modification time, or permissions differ; a directory or symlink is changed only when its recorded type differs. It does not mean a content diff is available.
_Avoid_: File diff, content change, move

**Difference Tree**:
The tree projection of file-tree changes that retains ancestor directories as navigation context and aggregates the changes below them.
_Avoid_: Flat change list

**Difference Availability**:
Whether a backup snapshot and its direct baseline are both available for comparison. When unavailable, the snapshot remains browseable but exposes no substituted comparison.
_Avoid_: Nearest-snapshot comparison, approximate diff

**Backup Block**:
A deduplicated logical data block referenced by one or more regular files in a backup snapshot.
_Avoid_: Upload attempt, pack
