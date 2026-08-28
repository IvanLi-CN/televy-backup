# Snapshot Detail Follows Snapshot Retention

Backup run logs remain lightweight execution summaries, while file and block details are available only for successful snapshots inside the target's configured snapshot retention window. The inspector uses the snapshot's own filemap and its direct baseline; it never preserves a second, independent history of full file lists or compares to a different retained snapshot when that baseline is unavailable.

## Considered Options

- Preserve full file lists and differences indefinitely in a separate run-history store.
- Compare a snapshot with the nearest retained predecessor when its direct baseline has expired.

The first option duplicates large, sensitive file metadata and introduces a second retention policy that can diverge from restore capability. The second produces a comparison that is not the backup's actual delta. Users who need a longer inspection history increase snapshot retention.
