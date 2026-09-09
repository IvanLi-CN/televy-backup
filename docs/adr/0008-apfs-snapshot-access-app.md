# ADR 0008: User Snapshot Access App

## Decision

Use a separate, windowless user app as the sole Full Disk Access process. It serves a versioned,
owner-UID-scoped Unix IPC that accepts configured target IDs and returns metadata pages or bounded
read streams. The backup daemon keeps only an opaque lease and logical source path. Because FDA does
not grant mount(2) privilege, the Access app delegates only mount, unmount, and UUID-scoped cleanup
to the separate minimal root mount helper defined in ADR 0009.

## Rationale

FDA is granted to a concrete app identity, while the root LaunchDaemon is restricted to the kernel
mount boundary and never reads user data. A separate Access app keeps file reading, CLI-only
scheduling, and user-visible FDA consent in the user session. No Apple Developer ID is required for
local use, but ad-hoc identity changes may require re-granting FDA after an update.

## Rejected alternatives

- A root backup daemon or root file reader: rejected because it expands authority beyond the mount
  operation. A minimal mount-only LaunchDaemon is accepted because `mount_apfs` requires that
  privilege even after FDA is granted.
- Full staging copy: rejected because it duplicates potentially large backup trees.
- Passing a snapshot mount path to the daemon: rejected because it bypasses the FDA boundary.

## Consequences

Strict mode fails closed when either the Access app/FDA or the mount helper is unavailable. The Access
app and root helper journal their respective resources, and settings show the exact app path, FDA
health, and mount-helper health.
