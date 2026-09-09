# Implementation

The implementation is split into four boundaries:

1. `crates/core` owns `BackupSource`, logical target identity, strict-mode
   configuration, and source-read completion.
2. `TelevyBackup Snapshot Access.app` owns snapshot reads and communicates over
   a versioned private socket.
3. The root mount helper owns only privileged APFS mount/unmount and cleanup.
4. `televybackupd`, the CLI, and Settings own scheduling, installation, status,
   and user-visible diagnostics.

The Access App and helper publish a release version and code identity. Strict
mode refuses mismatched components. Journal records are owner-scoped and
recovered idempotently; foreign or unrecorded snapshots are never deleted.

The source adapter materializes enough scan metadata and encrypted chunks to
release the lease before upload and index publication. Remote records continue
to use the configured logical source path.
