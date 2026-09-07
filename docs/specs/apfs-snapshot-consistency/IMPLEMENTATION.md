# Implementation

## Components

- `televybackup-snapshot-helper`: macOS root LaunchDaemon and restricted Unix socket protocol.
- `televybackupd`: user-side lease client, per-volume scheduling, and status projection.
- `televy_backup_core`: logical/physical source separation, filesystem boundary checks, and source-read-complete callback.
- CLI, SwiftUI Settings, and macOS package scripts: helper lifecycle and volume settings.

## Validation

- Unit-test command argument validation, peer UID checks, journal recovery, config persistence, and scheduler coalescing.
- Run a controlled macOS APFS integration test that creates a marker, snapshots, mutates the live marker, reads through the mount, and cleans by recorded UUID.
- Use the existing deterministic Settings demo state for UI evidence.

