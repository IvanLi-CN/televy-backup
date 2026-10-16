# Implementation

- Installer startup polling uses a monotonic 20-second window with 250ms sleeps clamped to the remaining time. Immediate connection refusal no longer consumes a fixed four-probe budget, slow calls consume the same clock, and a late ready reply is rejected. Existing component/build and installed-file hash checks remain mandatory. Deterministic clock tests and a hosted mutation of the old four-probe loop cover the regression; the macOS installer fixture can hold a disposable SQLite journal lock to delay the real helper.
- Snapshot failure classification retains the fixed `mount_permission_denied` reason when sanitized diagnostics identify the mount phase and `mount_apfs` permission denial. This distinguishes mounting from generic source access without forwarding paths or arbitrary command output to the UI.

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
- `macos/TelevyBackupApp/SnapshotAccessLaunchAgent.swift`: validates the running Access code with Security's dynamic validity check and its canonical bundle path. Direct registration holds a private per-user no-follow advisory lock across all lifecycle commands to serialize application instances, inspects the owned job before bootstrap, retains the current process, and requires a stopped producer plus a verified idle Access status and unchanged PID before restarting stale code. This implements ADR 0016's existing lifecycle boundary; no new architecture decision or helper artifact change is required.
- `crates/cli/src/snapshot_mount_service.rs`: explicit administrator-authorized mount-helper
  LaunchDaemon installation and status contract. Installation acknowledges success only after a bounded IPC probe confirms the current mount-helper component, protocol and candidate source commit, and the installed binary SHA-256 matches the candidate. Legacy Status without `build_commit` remains readable but cannot acknowledge a new installation.
- `macos/TelevyBackupApp/SnapshotMountHelperInstallation.swift`: coordinates administrator installation, renewed user-level Access registration, and the subsequent Access status check; preserves installation and registration errors across refresh, and rejects an exit-zero installation whose communication check still fails.
- `crates/daemon/src/control_ipc.rs` queries root Status independently of the approved Access artifact and exposes reachability, candidate readiness and build identities separately. `SnapshotMountHelperReadiness.swift` also binds readiness to the current app bundle, conservatively accepts legacy JSON as update-required, and supplies the production Settings update action. Final installation confirmation uses this same predicate. Status refresh remains read-only and no privileged lifecycle command is added.
- `SnapshotMountHelperStatusView.swift` is shared by Settings and the GitHub-hosted synthetic renderer. Hosted tests cover a reachable same-version old helper, missing/malformed identity, a stale daemon ready flag, a verified replacement and post-install rejection. `test-helper-readiness-regression.sh` proves the tests reject the old connectivity-only rule by changing a private copy of the production predicate, displaying the exact mutation, and checking the specific failure before running the unchanged candidate. This is a deterministic readiness regression, not a full backup acceptance.
- `scripts/macos/snapshot-helper-upgrade-integration-test.py`: dispatch-only GitHub-hosted regression retains a real rc.114 Access LaunchAgent through an in-place rc.120 artifact replacement, reproduces the protocol mismatch, and exercises candidate production registration and installation coordination to restore communication. It validates actual dynamic identity, healthy-process retention, and service cleanup with empty synthetic configuration. Initial service checks require confirmed absence and reject inspection errors. Path-absence checks include dangling symlinks. Cleanup confirms both services are absent before deleting fixture-owned targets and preserves the targets if shutdown cannot be verified; it does not claim APFS/FDA acceptance.
- `scripts/macos/mount-helper-install-integration-test.py`: dispatch-only GitHub-hosted macOS regression using real published helper binaries and a synthetic historical service, empty configuration, and isolated journals. It reproduces the protocol error before installation and checks communication after replacement without snapshots or TCC changes.
- `crates/snapshot-helper/src/mount_helper.rs`: source volume queries use the kernel device from a pinned no-follow directory metadata descriptor. The non-root authorization child must open the same device/inode, and reservation rechecks the source identity after authorization and inventory collection. System command errors carry fixed stage/category, exit status, channel and an allowlisted diagnostic summary; arbitrary stdout/stderr and plist parser details are never echoed.
- `scripts/macos/test-snapshot-source-reservation.py` and the default-off `snapshot_source_validation` dispatch input in `CI (main)`: hosted macOS creates a disposable APFS image, reproduces the baseline nested-directory `diskutil` exit 1 twice through production Reserve, then checks candidate Reserve, an explicitly empty inventory and security negatives. The same lane tests helper/CLI/daemon/core contracts and candidate installation with byte-identical approved Access. No `tmutil`, snapshots, FDA changes, owner data or local application execution is involved. Full backup completion remains a separate post-release observation.
