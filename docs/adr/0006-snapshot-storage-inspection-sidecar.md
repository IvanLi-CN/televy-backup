# Snapshot Storage Inspection Sidecar

## Context

The retained snapshot filemap identifies logical blocks, while the endpoint or materialized
dedupe catalog maps those blocks to physical Pack or Direct documents. Repeating that join,
parsing every mapping, grouping in memory, and sorting all objects for every Storage page makes
large snapshot inspection slow even though all of the source data is local.

The product must retain on-demand filemap materialization: a missing retained filemap is fetched
from remote storage automatically when an operator opens details. That remote transfer is not a
Telegram inventory lookup and must not be confused with local Storage aggregation.

## Decision

Use a local, snapshot-scoped SQLite sidecar at:

`index/storage-inspection/<endpoint-id>/<snapshot-id>.sqlite`

The sidecar stores a versioned complete-object summary table and unique block/slice membership
table. Membership references an integer sidecar object key; only the stable opaque storage ID is
persisted or returned to consumers. It contains no provider/object locator, chat, message,
document ID, manifest, or remote pointer.

Schedule a sidecar build after a successful daemon backup, without delaying the successful run
result. For older retained snapshots, build a missing sidecar only when Storage is requested. Both
paths use the already selected local snapshot filemap and endpoint or materialized dedupe catalog.
Build into a sibling temporary file and rename it only after all schema, metadata, object, and
membership writes commit. Readers open only the final complete file. Failed files are removed;
stale temporary files are reclaimed before a later build.

Storage pages and object expansions query the final sidecar with cursor-bound SQL. They do not
scan, group, or sort all snapshot mappings at request time. A retained sidecar survives normal
inspection requests and is deleted when retention prunes the corresponding snapshot. It is never
uploaded or remote-synchronized.

Source-filemap preparation and Storage-sidecar preparation are separate operations. The App
starts or joins a shared source preparation operation, accurately showing a remote snapshot-map
download when needed. It begins or observes local Storage indexing only after the source map is
ready. Neither state causes a snapshot to be described as unavailable.

## Consequences

- Repeated Storage pages and expansions are bounded local SQLite reads rather than full Rust
  materialization of a large snapshot.
- Preparation waits while the daemon has active backup activity, so a backfill does not compete
  with backup uploads or dedupe publication.
- The first Storage visit for a legacy retained snapshot can take time, but does not delay Summary,
  Files, or Blocks and never exposes partial results.
- Old `storage_objects` omissions remain unknown document metadata. The sidecar never derives or
  remotely fills physical document size or record time.
- Local cache size grows with retained snapshot Storage sidecars and is reclaimed by the same
  snapshot retention lifecycle, without a second user-facing quota setting.
