# WebDAV Snapshot Browse Contract

## Endpoint

Each Snapshot Browse Session exposes one effective WebDAV root:

```text
http://127.0.0.1:<os-assigned-port>/<capability>/
```

`<capability>` is an opaque, 256-bit, URL-safe session secret. It is not an HTTP Basic credential, has no persisted representation, and is valid only while its Snapshot Browse Session exists. Any other host, port, or path is not part of this interface.

## Resource Model

- The effective root is a collection whose Snapshot Directory children each represent one retained Backup Snapshot of the selected Backup Target.
- A Snapshot Directory maps names to one immutable file-tree view. Its displayed timestamp uses the Mac's active timezone at catalog insertion and includes a short snapshot ID.
- `TelevyBackup Diagnostics/Unavailable Entries.json` is a read-only diagnostic resource for entries whose recorded metadata is insufficient for faithful presentation. It is not backup content and is reserved at the volume root.
- Request paths are percent-decoded once, validated as forward-slash-separated snapshot-relative paths, and rejected when malformed, traversal-capable, or outside the effective root.

## Methods

| Method | Supported behavior |
| --- | --- |
| `OPTIONS` | Advertises the read-only WebDAV capability. |
| `PROPFIND` | Returns `207 Multi-Status` for an existing collection or entry at depth `0` or `1`. Listing a collection reflects currently retained snapshots and visible session overlay metadata. |
| `GET` | Returns a regular file only after resolving, decrypting, and verifying its requested data. A valid single byte range returns `206 Partial Content`; an unsatisfiable range returns `416`. |
| `HEAD` | Returns the metadata and range semantics of `GET` without a body. |
| `PUT`, `DELETE` | Return `405 Method Not Allowed` except for an explicit, unoccupied Finder-metadata allowlist backed by the session-memory overlay. |
| `MOVE`, `COPY`, `MKCOL`, `PROPPATCH`, `LOCK`, `UNLOCK` | Always return `405 Method Not Allowed`. |

The metadata overlay may include only test-proven Finder compatibility resources such as an unoccupied `.DS_Store` or AppleDouble sidecar. It is discarded when the session ends and cannot replace a real snapshot resource.

## Errors and Retention

- A Snapshot Directory no longer in the Snapshot Retention Window returns `404 Not Found`, including when it appeared in an earlier root listing.
- Missing, unreachable, corrupt, undecryptable, or integrity-invalid data returns a read failure. The service never returns an unchecked substitute.
- Requests without the exact capability path return `404 Not Found`, rather than an authentication challenge or capability hint.
