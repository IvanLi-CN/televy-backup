# Offline Storage Object Inspection

## Context

Snapshot inspection currently exposes logical file blocks, while a retained snapshot can also
reference physical Telegram documents: direct documents and pack documents containing slices. The
local index is the only durable authority available to the product without turning inspection into
a remote Telegram inventory query.

## Decision

Add a `storage_objects` table to the endpoint and materialized remote-dedupe indexes. A successful
upload records the normalized provider/object identity, whether it is a `direct` or `pack` document,
the exact uploaded payload byte count, and the record timestamp. The storage object identity exposed
through inspection is a stable opaque ID derived from the provider/object pair; raw Telegram locator
fields are never returned to the CLI, daemon IPC, or SwiftUI.

Storage inspection is snapshot-scoped. It groups only the selected snapshot's `chunk_objects` rows,
coalescing pack slices into one physical pack object and keeping direct objects separate. Expired
snapshots and retention semantics remain unchanged. When a legacy mapping has no `storage_objects`
row, document size and record time are returned as unknown; the product never derives a Telegram
document size from logical bytes or pack slices and never queries Telegram to fill the gap.

The endpoint index remains the source when remote dedupe is disabled. When remote dedupe is enabled,
the already materialized dedupe index is used for both chunk mappings and physical object metadata so
inspection has one consistent local source of truth.

## Consequences

- New uploads are auditable offline with exact payload size and write time.
- Legacy backups remain inspectable without migration-time network access, but show an explicit
  "not recorded" state for physical metadata.
- The object view is bounded and pageable like the existing file and block views; expanding an object
  reveals only its block slices referenced by the selected snapshot.
- This does not create a cross-snapshot remote inventory, expose chat/message/document fields, or
  change backup, restore, packing, or retention behavior.
