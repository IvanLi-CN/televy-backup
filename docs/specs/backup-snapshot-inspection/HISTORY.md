# Backup Snapshot Inspection History

> This document records topic-local compatibility and background. Durable rationale belongs in `docs/adr/`; the specification remains the authority for required behavior.

## Lifecycle and Compatibility

- The topic adds inspection to existing target-scoped run history without changing how backup, restore, or verify runs are executed.
- It must support both current two-level snapshot filemaps and legacy single-index snapshots through the existing resolver path.

## Background

- Run logs deliberately retain execution summaries instead of complete file lists.
- Snapshot retention is the single authority for detailed file and block availability; this avoids a divergent permanent history store.
- Direct-baseline comparison is required to preserve the meaning of a backup's delta.
- Physical Storage inspection is additive: successful uploads record exact document bytes and record time in the local endpoint/dedupe index, while legacy mappings remain inspectable with unknown physical metadata.
- The Storage view is snapshot-scoped and does not create a permanent remote inventory or expose Telegram locator fields. See [Offline storage object inspection](../../adr/0005-offline-storage-object-inspection.md) for the boundary rationale.
- The Storage tab has deterministic light and dark Main Window demo evidence covering recorded and legacy physical metadata, logical/document size separation, and expanded pack slices.
- Storage paging now relies on a retained-snapshot local sidecar so a repeated object page or expansion does not repeat full catalog aggregation. The sidecar is intentionally disposable and follows retained-snapshot cleanup; see [Snapshot storage inspection sidecar](../../adr/0006-snapshot-storage-inspection-sidecar.md).
- Filemap materialization remains automatic when a retained snapshot is opened. The owner-visible App state distinguishes that remote snapshot-map download from the subsequent local Storage-index preparation.

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
- [Snapshot detail follows snapshot retention](../../adr/0001-snapshot-inspection-retention.md)
