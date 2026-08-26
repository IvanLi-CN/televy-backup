# Backup Snapshot Inspection Implementation Status

> The specification in `./SPEC.md` defines the required behavior. This document records delivery coverage and rollout facts.

## Current Status

- Implementation: complete pending visual evidence and PR convergence
- Lifecycle: active
- Catalog note: implemented

## Delivered Coverage

- `SnapshotInspector` resolves current snapshot filemaps, current two-level index caches, and legacy single-index snapshots. It calculates direct-baseline changes and distinct logical block pages with request-bound, versioned cursors.
- `televybackup --json snapshots inspect summary|files|blocks` materializes a retained filemap only when absent locally, then exposes the stable JSON contract to the App.
- Activating a history row opens an inline run detail route. Successful backup runs load Summary, Files, and Blocks; unsuccessful or missing-snapshot runs remain summary-only.
- Files use a native `NSOutlineView` for lazy tree expansion and `NSTableView` for list/block pages. New, deleted, and changed states have SF Symbol icons and text labels.
- Rust covers current/legacy, first, direct-baseline-unavailable, empty legacy, logical block aggregation, cursor binding, and retention's preserved direct-baseline reference. Swift covers history eligibility and the app's existing UI isolation checks.

## Implementation Boundaries

- The App does not query SQLite directly.
- The implementation does not expand the retention window or retain independent file history. It preserves a pruned direct `base_snapshot_id` solely so retained descendants can report `baselineUnavailable` instead of becoming indistinguishable from first snapshots.
- The direct baseline remains the only comparison authority.

## Remaining Gaps

- Deterministic light and dark Main Window captures remain required before the feature can be declared visually complete.
- Fast-track PR creation, CI/review convergence, and final implementation status update remain required.

## References

- `./SPEC.md`
- `./HISTORY.md`
