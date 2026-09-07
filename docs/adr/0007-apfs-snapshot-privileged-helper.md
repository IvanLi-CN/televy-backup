# ADR-0007: Privileged APFS Snapshot Helper

## Status

Accepted

## Context

The backup scanner currently opens files from the live source directory. APFS snapshot creation can provide a stable filesystem view, but mounting and lifecycle management require privileged operations. The existing user daemon must retain user Keychain access and must not become a root process.

## Decision

Add a separate root `LaunchDaemon` with a minimal Unix IPC surface. It invokes only the supported macOS command chain `tmutil localsnapshot`, `mount_apfs -s`, and `diskutil apfs`. It never reads file contents or user secrets. The user daemon receives a private read-only mount lease and performs all scanning, encryption, indexing, and uploading.

The helper records each lease and snapshot UUID in a root-owned journal. It serializes operations per Volume UUID, cleans only its own UUIDs, and fails closed when `tmutil` output cannot be uniquely attributed. The app uses an explicit administrator installer because the current ad-hoc distribution cannot rely on notarized `SMAppService` LaunchDaemon installation.

## Consequences

- Enabled runs obtain filesystem-time consistency without requiring a password per backup.
- A failed or unsupported volume cannot silently fall back to a live scan.
- `tmutil localsnapshot` may create snapshots for all Time Machine-included APFS volumes; the helper tracks and cleans the complete invocation manifest.
- Database transaction consistency remains outside this ADR.

