CREATE TABLE IF NOT EXISTS storage_objects (
  provider TEXT NOT NULL,
  object_id TEXT NOT NULL,
  storage_id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('direct', 'pack')),
  document_bytes INTEGER NOT NULL,
  recorded_at TEXT NOT NULL,
  PRIMARY KEY (provider, object_id),
  UNIQUE (provider, storage_id)
);

CREATE INDEX IF NOT EXISTS idx_storage_objects_provider_kind
  ON storage_objects(provider, kind, storage_id);

-- Storage inspection resolves snapshot chunk hashes back to physical objects.
-- Keep that lookup indexed even on older dedupe databases that predate this view.
CREATE INDEX IF NOT EXISTS idx_chunk_objects_chunk_hash
  ON chunk_objects(chunk_hash, provider, object_id);
