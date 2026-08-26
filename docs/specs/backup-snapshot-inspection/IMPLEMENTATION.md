# Backup Snapshot Inspection Implementation Status

> The specification in `./SPEC.md` defines the required behavior. This document records delivery coverage and rollout facts.

## Current Status

- Implementation: not started
- Lifecycle: active
- Catalog note: planned

## Planned Coverage

- Core resolves retained snapshot filemaps and computes bounded summary, file, difference, and logical-block pages.
- CLI exposes the read-only JSON inspector contract; the macOS App consumes it on background work queues.
- Main Window replaces a selected history row with a native run-detail route and virtualized Files/Blocks surfaces.
- Unit, contract, Swift UI-state, and deterministic visual evidence cover success, failure, retention, and large-list behavior.

## Implementation Boundaries

- The App does not query SQLite directly.
- The implementation does not alter backup storage or retention behavior.
- The direct baseline remains the only comparison authority.

## Remaining Gaps

- All implementation and validation work remains open.

## References

- `./SPEC.md`
- `./HISTORY.md`
