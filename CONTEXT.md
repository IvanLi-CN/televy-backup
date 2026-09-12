# TelevyBackup

TelevyBackup protects local data in remote storage and restores protected data back to a local destination. Its macOS client presents transfer activity and controls the local backup environment.

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

## Snapshot Browsing

**Backup Target**:
The configured local source and remote storage binding whose backup runs produce one sequence of Backup Snapshots.
_Avoid_: Folder, backup job

**Backup Target Browse Volume**:
A Finder-visible, read-only projection of the Backup Snapshots currently retained for one Backup Target. It is not a remote share and it is not itself a Backup Snapshot.
_Avoid_: Network disk, mounted snapshot

**Snapshot Directory**:
The time-named top-level directory in a Backup Target Browse Volume that represents exactly one Backup Snapshot. Its display name is session-local; its embedded short snapshot ID disambiguates it.
_Avoid_: Backup folder, snapshot mount

**Snapshot Browse Session**:
The temporary local access session that owns one Backup Target Browse Volume and ends when it is explicitly unmounted or reclaimed during application recovery.
_Avoid_: Restore session, mount lease

**Loopback Snapshot Browsing Service**:
The local-only WebDAV representation of one Snapshot Browse Session. It is reached only through a one-time capability URL on the current Mac.
_Avoid_: Remote WebDAV server, shared WebDAV service

## GUI Lifecycle

**GUI Controller**:
The macOS app instance that presents and controls one local backup environment. It is separate from the daemon that executes scheduled and queued work.
_Avoid_: Daemon, backup worker

## Installation And Privilege Model

**User-Visible Product**:
The single `TelevyBackup.app` bundle a person downloads, moves, and updates. No second top-level companion application is presented as a separate installation or update task.
_Avoid_: Two-app installation, standalone companion app

**Private Access Helper**:
A nested application bundle shipped inside the User-Visible Product. It runs only to perform the Full Disk Access-scoped snapshot operation, has no Finder-facing installation workflow, and remains a distinct macOS authorization identity from the GUI Controller.
_Avoid_: Second product, GUI Controller

**Authorization-Stable Helper Artifact**:
The exact previously authorized Private Access Helper bundle, including its ad-hoc signature. A product-only release reuses its bytes unchanged so its macOS code identity remains unchanged.
_Avoid_: Rebuilt helper, product-versioned helper

**Helper Migration**:
The one-time move from the legacy externally installed Snapshot Access application to the Private Access Helper's stable path inside the User-Visible Product. It can require the person to grant Full Disk Access once to the new authorization identity.
_Avoid_: Routine product update

**System Mount Helper**:
The root-only mount component installed at its stable system path as part of the User-Visible Product's setup. It is not a separate user product; changing it remains an explicit administrator-authorized and, when required, Full Disk Access migration.
_Avoid_: User-installed second app, user-session file reader

**Menu-bar Agent**:
The default macOS application identity while TelevyBackup has no persistent top-level window. It keeps `LSUIElement=true` semantics: the app remains available from the status item without appearing in the Dock or `Cmd-Tab`.
_Avoid_: Hidden application, daemon

**Persistent Top-level Window**:
A main or settings window, plus a future window explicitly registered with the window activation coordinator. Popovers, sheets, file panels, and alerts do not change application identity or count toward this window set.
_Avoid_: Any visible panel, popover

**Persistent Window Activation**:
The runtime policy that changes the app from `.accessory` to `.regular` before a persistent window is shown, keeps `.regular` while a persistent window is minimized, and returns to `.accessory` after the last persistent window closes.
_Avoid_: Window focus, daemon activation

**GUI-only Exit**:
An orderly end of a GUI Controller for one backup environment while its daemon, scheduled work, and daemon-owned queue remain available.
_Avoid_: Complete exit, stop backup

**Complete Exit**:
An orderly end of the GUI Controller that also stops the daemon for the selected backup environment and cancels GUI-owned local jobs.
_Avoid_: GUI-only exit, close window

**GUI-owned Local Job**:
A command process started directly by the GUI and owned by that GUI's lifecycle. It is distinct from daemon work, including a daemon that the GUI started as a local fallback.
_Avoid_: Daemon task, backup queue member

**Backup Quick Action**:
The menu-bar action that requests one daemon-managed backup batch for all enabled targets in the current backup environment.
_Avoid_: Per-target backup
