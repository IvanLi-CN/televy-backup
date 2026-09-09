# ADR 0008: Minimal Root APFS Mount Helper

## Context

On the supported macOS path, a separate privileged boundary is required for
mounting and UUID-exact cleanup. The helper must not become a second backup
daemon.

## Decision

Install one product-managed root `LaunchDaemon` whose only API is a restricted
mount lifecycle for a validated lease. It accepts only the peer user's UID,
validated volume and recorded snapshot identity, uses fixed system tools, and
stores an owner-scoped journal. It performs no file reads, encryption, upload,
Keychain access, or network I/O.

## Consequences

Initial installation and updates require administrator approval. Scheduled
backups do not. The final helper artifact also receives FDA and is included in
the full-chain verification because the product cannot claim a working chain
from installation or connectivity alone.
