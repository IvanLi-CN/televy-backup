# Backup Target WebDAV Browsing

> This file is the durable topic requirements contract. Current implementation facts belong in `IMPLEMENTATION.md`; lifecycle and change references belong in `HISTORY.md`.

## Context and Scope

- Context: Operators need to inspect and copy files from multiple retained Backup Snapshots of one Backup Target directly in Finder without first restoring a complete snapshot to a destination directory.
- In scope: A read-only Backup Target Browse Volume, its local-only WebDAV transport, Finder mount lifecycle, remote-catalog freshness, on-demand verified reads, and bounded local caching.
- Out of scope: A LAN or Internet WebDAV server, remote-storage protocol replacement, source-volume mounting, backup retention changes, write-back, full metadata preservation, and content diffs.

## Terms and Interfaces

- `Backup Target Browse Volume`: The Finder volume defined in [CONTEXT.md](../../../CONTEXT.md). One volume represents all retained Backup Snapshots for exactly one Backup Target.
- `Snapshot Directory`: A root collection for one Backup Snapshot. It is named from the snapshot time in the computer's active timezone when it enters the session catalog, plus a short snapshot ID.
- `Snapshot Browse Session`: The temporary session that owns the service capability, mount state, and session-local Finder metadata overlay.
- `Snapshot Content Reader`: The core read boundary that prepares a retained snapshot, resolves metadata, lists directories, and returns a verified byte range of one regular file.
- Interface: [WebDAV contract](./contracts/webdav.md) and [daemon control IPC contract](./contracts/control-ipc.md).

## Requirements

### REQ-WDB-001

- The system MUST create one Backup Target Browse Volume for one explicitly selected Backup Target, not one volume per Backup Snapshot.
- The volume root MUST project every Backup Snapshot currently in that target's Snapshot Retention Window as a Snapshot Directory.
- The system MUST NOT substitute a different target, an unretained snapshot, or a source-volume snapshot.

### REQ-WDB-002

- Before a new Snapshot Browse Session is created, the system MUST attempt to refresh the selected target's remote snapshot catalog.
- If that refresh fails but a local catalog exists, the system MUST require an explicit operator confirmation before mounting the cached catalog and MUST identify the catalog as cached in the mount initiation result.
- Telegram private-chat targets do not support the existing pinned bootstrap catalog protocol; for those targets remote-first refresh is reported as unavailable and the same explicit cached-catalog confirmation is required.
- If neither a fresh nor a usable cached catalog exists, the system MUST NOT create a volume.

### REQ-WDB-003

- A Snapshot Directory name MUST show its snapshot time in the computer's active timezone without a timezone suffix and MUST include a short snapshot ID to make the name unambiguous.
- A directory name MUST remain stable for the lifetime of its Snapshot Browse Session. The same Backup Snapshot may receive a different displayed local time in a later session after a timezone change.
- Root listings MUST reflect the current Snapshot Retention Window. A snapshot pruned after a listing is returned MUST resolve as absent for later access and MUST NOT be retained merely because a volume is mounted.

### REQ-WDB-004

- The Snapshot Content Reader MUST provide prepared-snapshot metadata lookup, direct-child directory listing, and regular-file byte-range reads without materializing the complete snapshot at a restore destination.
- A WebDAV `GET` or `HEAD`, including an HTTP byte-range request, MUST resolve only the requested logical file bytes. It MAY require full retrieval of an underlying encrypted object or Pack because the current storage boundary has no remote range-read capability.
- Bytes returned to WebDAV MUST be decrypted and verified against their recorded chunk hash before they are exposed. A missing, corrupt, undecryptable, or unreachable object MUST produce an access error and MUST NOT yield fabricated or unchecked content.

### REQ-WDB-005

- Snapshot data is immutable. The service MUST reject mutations of Backup Snapshot paths, including `PUT`, `DELETE`, `MOVE`, `COPY`, `MKCOL`, `PROPPATCH`, `LOCK`, and `UNLOCK`, with a method-not-allowed response.
- The service MAY accept a narrow, test-proven allowlist of Finder metadata writes such as an unoccupied `.DS_Store` or AppleDouble sidecar. Those writes MUST live only in a session-memory overlay, MUST be discarded when the session ends, and MUST NOT mask or alter a real snapshot entry.
- Every request path MUST be decoded and validated as a snapshot-relative path. Traversal, malformed encoding, and paths outside the session capability root MUST be rejected.

### REQ-WDB-006

- The Loopback Snapshot Browsing Service MUST bind only `127.0.0.1` on an OS-assigned port and MUST use a unique 256-bit in-memory capability path for each Snapshot Browse Session.
- The capability URL MUST be delivered only over the existing authenticated daemon control boundary to the mount initiator. It MUST NOT be persisted in configuration, Keychain, cache metadata, normal UI, status responses, or logs. macOS `mount_webdav` necessarily receives the URL as a transient launch argument; this OS process-table visibility is the explicit platform exception for Finder mounting and MUST NOT be copied into application logs or durable state.
- Requests that omit or mismatch the capability path MUST return not found. The service MUST NOT listen on a LAN, public, or Unix-socket WebDAV endpoint.

### REQ-WDB-007

- The system MUST maintain an encrypted-object disk LRU cache with a default capacity of `20 GiB` and an operator-configurable capacity. It MUST evict before adding objects where possible and MUST coalesce concurrent retrievals of the same object.
- Decrypted content MUST be memory-only and bounded. The cache MUST NOT persist plaintext file bytes or decrypted chunks.
- Cache quota, temporary download failure, and remote-object size constraints MUST be reported as readable access errors without corrupting an existing cache entry.

### REQ-WDB-008

- The volume MUST faithfully expose the recorded names, hierarchy, regular-file bytes, size, modification time, and directory structure for retained entries within the supported metadata model.
- A historical symlink whose target was not recorded MUST NOT be presented as a regular file, blank file, or valid Finder link. It MUST be reported as unavailable through a read-only diagnostic collection.
- The volume MUST NOT claim preservation of symlink targets, extended attributes, ACLs, owner/group identity, creation time, or unsupported special-file semantics when the Backup Snapshot does not record them.

### REQ-WDB-009

- The macOS client MUST offer a target-scoped action that creates the session, mounts the returned capability URL as a Finder volume, and opens that volume.
- A mounted volume MUST remain available without an idle timeout. Finder ejection or an explicit application unmount MUST release the corresponding Snapshot Browse Session and revoke its capability.
- Application startup and daemon recovery MUST end and clean up every existing Snapshot Browse Session and its mount before creating replacement sessions.

## Verification

### VER-WDB-001

- Method: Core fixtures with multiple targets, retained and pruned snapshots, duplicate local-time values, and a remote-catalog refresh failure.
- covers: `REQ-WDB-001`, `REQ-WDB-002`, `REQ-WDB-003`
- Pass condition: The selected target produces only its retained Snapshot Directories; the fresh, confirmed-cached, and unavailable catalog states are distinguishable; a pruned snapshot cannot be read after removal.

### VER-WDB-002

- Method: Core reader fixtures containing nested regular files, direct objects, Pack slices, encrypted corruption, and HTTP Range requests.
- covers: `REQ-WDB-004`, `REQ-WDB-007`
- Pass condition: Directory and range responses map to the expected bytes; duplicate object requests coalesce; bad ciphertext/hash data and cache-quota errors return failures with no unchecked response or plaintext disk cache.

### VER-WDB-003

- Method: WebDAV protocol integration tests against the loopback service, including percent-encoded names, traversal attempts, wrong capabilities, write methods, and allowed Finder metadata paths.
- covers: `REQ-WDB-005`, `REQ-WDB-006`, `REQ-WDB-008`
- Pass condition: The server is reachable only at the session capability URL, rejects unsupported methods and invalid paths, does not expose secrets, preserves real snapshot entries, and emits only supported metadata plus explicit unavailable-entry diagnostics.

### VER-WDB-004

- Method: macOS integration tests that use `mount_webdav` and Finder against a deterministic local snapshot fixture, followed by explicit eject, application restart, and daemon-recovery scenarios.
- covers: `REQ-WDB-005`, `REQ-WDB-006`, `REQ-WDB-009`
- Pass condition: Finder can enumerate and copy regular files, requested metadata writes stay session-local, no idle period closes a mounted volume, and eject or recovery revokes the mount and capability.

The repository provides the daemon-backed portion of this acceptance as an ignored macOS test. Run `TELEVYBACKUP_RUN_WEBDAV_MOUNT_ACCEPTANCE=1 scripts/macos/verify-webdav-snapshot-browsing.sh` on a real macOS session with the required Full Disk Access grant; the normal CI invocation intentionally reports this real-mount evidence as pending rather than substituting a daemon unit test for it.

## Related ADRs

- [0001-snapshot-inspection-retention](../../adr/0001-snapshot-inspection-retention.md)
- [0010-loopback-webdav-snapshot-browsing](../../adr/0010-loopback-webdav-snapshot-browsing.md)

## Visual Evidence

- None

## References

- [WebDAV contract](./contracts/webdav.md)
- [Daemon control IPC contract](./contracts/control-ipc.md)
- [Implementation status](./IMPLEMENTATION.md)
- [Topic history](./HISTORY.md)
- [Backup Snapshot Inspection](../backup-snapshot-inspection/SPEC.md)
- [Remote-first index sync](../remote-first-index-sync/SPEC.md)
