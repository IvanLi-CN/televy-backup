# ADR 0007: Brokered APFS Snapshot Reads

## Context

The backup must read one filesystem time point while the live source changes.
The daemon also owns secrets and uploads, so granting it filesystem access to a
snapshot mount would widen the privileged boundary.

## Decision

Use a separate user-level `TelevyBackup Snapshot Access.app` with FDA. It
creates snapshots and is the only component that enumerates and reads the
mounted snapshot. The daemon requests target-scoped leases and bounded pages or
byte streams through private IPC. It never receives a mount path.

## Consequences

The logical source identity remains stable, while physical snapshot paths stay
private. Access App replacement or ad-hoc code-identity changes may require a
new FDA grant. Strict mode fails closed when the service is unavailable.
