# ADR 0008: User Snapshot Access App

## Decision

Use a separate, windowless user app as the user-session file-reading Full Disk Access process. It
serves a versioned, owner-UID-scoped Unix IPC that accepts configured target IDs and returns metadata
pages or bounded read streams. The backup daemon keeps only an opaque lease and logical source path.
The current validated strict-mode baseline grants FDA to the exact Access app identity and to the
exact root mount-helper identity. The Access app is embedded in the single visible
`TelevyBackup.app` and registered by `SMAppService` using a bundle-relative program. Because FDA does not grant mount(2) privilege, the Access app
delegates only mount, unmount, and UUID-scoped cleanup to the separate minimal root helper defined
in ADR 0009.

## Rationale

FDA is granted to a concrete code identity. The root LaunchDaemon remains restricted to the kernel
mount boundary and never reads user data, even though the current validated baseline grants it FDA.
A separate Access app keeps file reading, CLI-only scheduling, and user-visible consent in the user
session. No Apple Developer ID is used. Ordinary main-app releases reuse the unchanged helper
artifact and do not require a new FDA grant; actual helper changes are explicit authorization
migrations.

## Rejected alternatives

- A root backup daemon or root file reader: rejected because it expands authority beyond the mount
  operation. A minimal mount-only LaunchDaemon is accepted because `mount_apfs` requires that
  privilege even after FDA is granted.
- Full staging copy: rejected because it duplicates potentially large backup trees.
- Passing a snapshot mount path to the daemon: rejected because it bypasses the FDA boundary.

## Consequences

Strict mode fails closed when either required FDA grant, the Access app, or the mount helper is
unavailable. The Access app and root helper journal their respective resources. Settings show the
embedded helper's exact path and only claim FDA observations it can actually make. The old external
registration is backed up and migrated transactionally; it is not silently treated as the current
helper.
