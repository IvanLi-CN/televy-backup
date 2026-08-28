# Snapshot Inspector CLI Contract

## Scope

The CLI provides a read-only, JSON-only inspection surface over retained backup snapshots for terminal users. The macOS App uses the daemon's local control IPC instead; the CLI remains an independent public contract.

## Commands

```text
televybackup --json snapshots inspect summary --snapshot-id <snapshot-id>
televybackup --json snapshots inspect files --snapshot-id <snapshot-id> --presentation <tree|list> --scope <all|changes> [--parent <relative-path>] [--query <text>] [--cursor <opaque>] [--limit <1..500>]
televybackup --json snapshots inspect blocks --snapshot-id <snapshot-id> [--changes-only] [--query <hash-prefix>] [--cursor <opaque>] [--limit <1..500>]
```

- `summary` returns immutable snapshot metadata, aggregate file/block statistics, and difference availability.
- `files` returns direct children for `presentation=tree` and flat entries for `presentation=list`.
- `scope=changes` is valid only when difference availability is `available` or `firstSnapshot`.
- Tree responses in changes scope include unchanged ancestor context rows and their aggregate descendant change counts.
- A cursor is opaque, scoped to the exact immutable query, and must not be reused after any snapshot, presentation, scope, parent, or query change.

## Summary Result

```json
{
  "snapshot": {
    "snapshotId": "snp_...",
    "createdAt": "2026-08-27T00:00:00Z",
    "sourcePath": "/Users/ivan/Projects",
    "label": "Projects",
    "baseSnapshotId": "snp_..."
  },
  "availability": {
    "state": "available",
    "reason": null
  },
  "files": {
    "entries": 1200,
    "regularFiles": 1100,
    "directories": 95,
    "symlinks": 5,
    "bytes": 123456789
  },
  "changes": {
    "state": "available",
    "added": 10,
    "deleted": 3,
    "changed": 4
  },
  "blocks": {
    "distinct": 730,
    "bytes": 123456789
  }
}
```

`availability.state` is `available`, `firstSnapshot`, or `baselineUnavailable`. `firstSnapshot` returns `null` as `baseSnapshotId` and treats every entry as added. `baselineUnavailable` permits all-files and block queries but rejects `scope=changes`.

## File Page Result

```json
{
  "entries": [
    {
      "path": "src/main.rs",
      "name": "main.rs",
      "kind": "file",
      "change": "changed",
      "isAncestorContext": false,
      "size": 200,
      "mtimeMs": 1724716800000,
      "mode": 33188,
      "baseline": { "kind": "file", "size": 180, "mtimeMs": 1724630400000, "mode": 33188 },
      "descendantChanges": { "added": 0, "deleted": 0, "changed": 0 }
    }
  ],
  "nextCursor": null
}
```

- `change` is `unchanged`, `added`, `deleted`, or `changed`.
- Deleted entries use baseline metadata as their primary metadata.
- `baseline` is omitted for added and unchanged rows.
- `descendantChanges` is populated for tree directory rows.

## Block Page Result

```json
{
  "entries": [
    {
      "hash": "blake3-hex",
      "size": 1048576,
      "changedFiles": 2,
      "referencingFiles": 3
    }
  ],
  "nextCursor": null
}
```

Rows are one per distinct logical chunk hash referenced by regular files. `referencingFiles` counts all regular files in the selected current snapshot that reference the block. `changedFiles` counts current regular files classified as `added` or `changed` against the direct baseline that reference the block; for a first snapshot, all current regular files count as changed. Deleted baseline files do not contribute to current block rows. `--changes-only` returns only rows with `changedFiles > 0` and is valid when the direct baseline is available or the snapshot is a first snapshot. The contract deliberately contains no upload-attempt, pack, remote object, or new-versus-reused field.

## Errors

All command errors use the existing CLI structured-error envelope. Required stable codes are:

| Code | Meaning | Retryable |
| --- | --- | --- |
| `snapshot.not_found` | No retained snapshot metadata matches the ID. | no |
| `snapshot.not_retained` | The run refers to a snapshot outside the configured retention window. | no |
| `snapshot.filemap_unavailable` | The retained filemap could not be found, downloaded, decrypted, or opened. | yes when transport-related |
| `snapshot.baseline_unavailable` | A changes query requested a retained snapshot without its direct baseline. | no |
| `snapshot.inspect.invalid_cursor` | The cursor is malformed or does not match the request. | no |
| `snapshot.inspect.invalid_argument` | Presentation, scope, parent path, query, or limit is invalid. | no |

## Compatibility

- The resolver supports a filemap DB with attached endpoint/dedupe data and legacy snapshot indexes containing their own chunk-object mapping.
- The UI must treat unknown additive JSON fields as ignorable and missing optional counts as unavailable rather than zero.
