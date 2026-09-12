# Backup Target WebDAV Browsing Implementation Status

> The specification in `./SPEC.md` defines the required behavior. This document records implementation coverage and rollout facts.

## Current Status

- Implementation: core and daemon path implemented; macOS client wiring implemented
- Lifecycle: active
- Runtime note: the feature is exposed through the existing control socket and remains opt-in from the target History view.

## Intended Boundaries

- `crates/core` owns `SnapshotContentReader`: retained-snapshot preparation, filemap metadata queries, chunk-overlap calculation, object retrieval, decryption, and integrity verification. It reuses the snapshot filemap and restore cryptographic rules without reusing whole-snapshot restore output.
- `crates/daemon` owns the target catalog refresh, Snapshot Browse Session registry, encrypted-object cache, loopback WebDAV service, and new authenticated control IPC methods. The service uses `dav-server 0.11` with a custom read-through `DavFileSystem`; it is private to the daemon, not a CLI JSON or public HTTP API.
- The macOS client owns the target-history action, fresh-versus-cached confirmation, invocation of `mount_webdav`, Finder opening, and explicit unmount. It never reads remote objects, filemaps, or cache databases directly.
- The current `SnapshotInspectionService` supplies related retained-filemap preparation and directory metadata behavior, but it is not a byte-content reader. The restore flow has verified chunk download/decrypt logic, but it currently writes an entire snapshot to a destination and is not the WebDAV request path.

## Requirement Coverage

| Requirement | Planned owner | Current state |
| --- | --- | --- |
| `REQ-WDB-001` to `REQ-WDB-003` | daemon catalog and session registry; macOS client | Implemented: one target session, local-time snapshot directories, duplicate-name disambiguation, cached-catalog fallback contract |
| `REQ-WDB-004` | core `SnapshotContentReader`; daemon WebDAV adapter | Implemented: direct and Pack object reads, verified ranges, read-only WebDAV GET/HEAD/PROPFIND |
| `REQ-WDB-005` and `REQ-WDB-006` | daemon WebDAV adapter and session registry | Implemented: loopback bind, opaque in-memory capability, path validation, mutation rejection, Finder metadata response |
| `REQ-WDB-007` | core encrypted-object cache | Implemented: atomic encrypted-object cache, serialized same-object retrieval, LRU quota eviction, configurable default |
| `REQ-WDB-008` | core metadata adapter and daemon diagnostic collection | Implemented: recorded file metadata and visible diagnostics resource for unavailable entries |
| `REQ-WDB-009` | macOS client, daemon lifecycle handling | Implemented: History/context actions, `mount_webdav`, Finder open, startup recovery, explicit/eject cleanup observer |

## Delivery Constraints

- The current storage abstraction downloads complete encrypted documents. HTTP Range support therefore improves the Finder-facing logical-file response without promising remote byte-range retrieval; Pack-backed chunks can require a complete Pack download.
- Existing retained snapshot filemaps contain regular-file and directory metadata plus symlink kind, but not a symlink target. Historical snapshots cannot gain that missing target retroactively.
- The existing daemon control socket is the sole control-plane boundary. A new WebDAV listener must not be repurposed as a general daemon API.
- A runtime implementation requires macOS Finder compatibility evidence before the feature can be exposed as a normal target-history action.

## Related Changes

- `crates/core/src/snapshot_browsing.rs`
- `crates/daemon/src/snapshot_browse.rs`
- `macos/TelevyBackupApp/MainWindow.swift`
- `macos/TelevyBackupApp/TelevyBackupApp.swift`
- `macos/TelevyBackupApp/SettingsWindow.swift`

## References

- `./SPEC.md`
- `./HISTORY.md`
- `../../../crates/core/src/snapshot_inspection.rs`
- `../../../crates/core/src/restore.rs`
- `../../../crates/daemon/src/snapshot_inspection_ipc.rs`
