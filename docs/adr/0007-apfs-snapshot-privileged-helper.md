# ADR 0007: APFS Snapshot Privilege Boundary (Superseded)

This historical decision is superseded by [ADR 0008](0008-apfs-snapshot-access-app.md). The
root LaunchDaemon and privileged helper described here are not part of the current product design.
Current implementations use the user-session Snapshot Access app, configured-target IPC, opaque
leases, and user-owned journal described by ADR 0008, together with the mount-only root boundary
described by ADR 0009. The old design's root process reading or uploading user data remains rejected.
