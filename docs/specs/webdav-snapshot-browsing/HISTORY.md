# Backup Target WebDAV Browsing History

> This file records topic-local lifecycle, compatibility, and background. Durable decision rationale remains in `docs/adr/`.

## Lifecycle / Compatibility

- This capability supplements restore and snapshot inspection with Finder browsing. It does not change the meaning or retention lifecycle of a Backup Snapshot.
- The topic supports retained historical snapshots with regular files and directories. Historical symlinks without a stored target remain explicitly unavailable rather than being converted to a different file type.

## Replacements / Background

- The Finder integration uses a loopback WebDAV service rather than a filesystem extension. The rationale and security boundary are recorded in [0010-loopback-webdav-snapshot-browsing](../../adr/0010-loopback-webdav-snapshot-browsing.md).

## Related Changes

- None

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
