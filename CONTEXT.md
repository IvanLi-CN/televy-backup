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

**Bidirectional Sync**:
A Menu Bar Activity State in which upload and download activities coexist, or a native sync activity declares both directions.
_Avoid_: Two-way backup

**Verification**:
A task that reads protected remote data to validate it without restoring that data to a local destination.
_Avoid_: Restore

**Current Task Failure**:
A failure emitted by a backup, restore, or verification task in the current live session. Historical run results do not determine the Menu Bar Activity State.
