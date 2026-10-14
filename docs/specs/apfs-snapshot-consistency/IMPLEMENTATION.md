# Implementation

- `crates/snapshot-helper`: user-session Snapshot Access protocol and journal-backed lifecycle,
  including recorded mount-device identities, migratable lease journals, reservation renewal while
  snapshot creation is in flight, unique firmlink resolution, and descriptor-relative no-follow
  reads. Missing or changed mount identities fail the current strict run before source bytes are
  served.
- `crates/snapshot-helper/src/mount_helper.rs` and `src/bin/televybackup-snapshot-mount-helper.rs`:
  root IPC, peer-UID source-path authorization, serialized helper-owned snapshot creation through `RecordCleanup`, UUID journal, crash recovery, and component version `0.5.0` validation that
  binds the requested snapshot name to its manifest UUID before `mount_apfs`, passes the validated
  `/dev/<device>` identity instead of a mutable request pathname, and verifies the mounted device
  afterward. Source-path authorization runs in an exec'd one-shot helper before the server runtime
  starts, and released leases remain as auditable tombstones; a later reserve audits their
  validated baseline and re-blocks the volume if inventory changed. Deletion clears only the
  successfully consumed manifest entries, retains the original reservation baseline, and only
  releases after the post-deletion inventory returns to that baseline; migrated rows without a
  validated baseline remain blocked. The installed `0.9.8` helper remains
  migration-readable only.
- `crates/daemon/src/snapshot_client.rs`: restricted IPC client and `BrokeredSnapshotSource`.
- `crates/core/src/backup.rs`: `BackupSource` seam shared by local and brokered reads.
- `crates/cli/src/snapshot_service.rs`: fixed-bundle status plus transactional legacy registration
  migration, owner renewal, and rollback; arbitrary external app installation and DMG-mounted
  production migration are not supported.
- `macos/TelevyBackupApp/SettingsWindow.swift`: per-volume FDA and service state presentation.
- `scripts/macos/build-app.sh`: nested Snapshot Access bundle and the canonical installed-app
  `Program` LaunchAgent plist used by direct `launchctl` registration.
- `scripts/macos/snapshot-access-launchagent-integration-test.sh`: isolated unique-label launchctl
  registration fixture that never starts the Snapshot Access helper.
- `macos/TelevyBackupApp/TelevyBackupApp.swift`: ad-hoc `launchctl` registration, optional
  `SMAppService` registration for signed builds, and migration commit/rollback orchestration.
- `macos/TelevyBackupApp/SnapshotAccessLaunchAgent.swift`: validates the running Access code with Security's dynamic validity check and its canonical bundle path. Direct registration inspects the owned job before bootstrap, retains the current process, and requires a stopped producer plus a verified idle Access status and unchanged PID before restarting stale code. This implements ADR 0016's existing lifecycle boundary; no new architecture decision or helper artifact change is required.
- `crates/cli/src/snapshot_mount_service.rs`: explicit administrator-authorized mount-helper
  LaunchDaemon installation and status contract. Installation acknowledges success only after a bounded IPC probe confirms the current mount-helper component and protocol; bootstrap alone is insufficient.
- `macos/TelevyBackupApp/SnapshotMountHelperInstallation.swift`: coordinates administrator installation, renewed user-level Access registration, and the subsequent Access status check; preserves installation and registration errors across refresh, and rejects an exit-zero installation whose communication check still fails.
- `scripts/macos/snapshot-helper-upgrade-integration-test.py`: dispatch-only GitHub-hosted regression retains a real rc.114 Access LaunchAgent through an in-place rc.120 artifact replacement, reproduces the protocol mismatch, and exercises candidate production registration and installation coordination to restore communication. It validates actual dynamic identity, healthy-process retention, and service cleanup with empty synthetic configuration; it does not claim APFS/FDA acceptance.
- `scripts/macos/mount-helper-install-integration-test.py`: dispatch-only GitHub-hosted macOS regression using real published helper binaries and a synthetic historical service, empty configuration, and isolated journals. It reproduces the protocol error before installation and checks communication after replacement without snapshots or TCC changes.
