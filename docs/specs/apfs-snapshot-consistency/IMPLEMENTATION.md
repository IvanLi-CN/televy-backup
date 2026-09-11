# Implementation

- `crates/snapshot-helper`: user-session Snapshot Access protocol and journal-backed lifecycle.
- `crates/snapshot-helper/src/mount_helper.rs` and `src/bin/televybackup-snapshot-mount-helper.rs`:
  mount-only root IPC, UUID journal, and crash recovery.
- `crates/daemon/src/snapshot_client.rs`: restricted IPC client and `BrokeredSnapshotSource`.
- `crates/core/src/backup.rs`: `BackupSource` seam shared by local and brokered reads.
- `crates/cli/src/snapshot_service.rs`: fixed-bundle status plus transactional legacy registration
  migration and rollback; arbitrary external app installation is not supported.
- `macos/TelevyBackupApp/SettingsWindow.swift`: per-volume FDA and service state presentation.
- `scripts/macos/build-app.sh`: nested Snapshot Access bundle and `SMAppService` LaunchAgent plist.
- `macos/TelevyBackupApp/TelevyBackupApp.swift`: SMAppService registration and migration commit/
  rollback orchestration.
- `crates/cli/src/snapshot_mount_service.rs`: explicit administrator-authorized mount-helper
  LaunchDaemon installation and status contract.
