DROP INDEX IF EXISTS idx_file_chunks_chunk_hash_file_seq;

CREATE INDEX IF NOT EXISTS idx_snapshots_source_created_at
  ON snapshots(source_path, created_at DESC);
