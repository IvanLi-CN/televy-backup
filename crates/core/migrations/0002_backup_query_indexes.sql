CREATE INDEX IF NOT EXISTS idx_files_snapshot_kind_file
  ON files(snapshot_id, kind, file_id);

CREATE INDEX IF NOT EXISTS idx_file_chunks_chunk_hash_file_seq
  ON file_chunks(chunk_hash, file_id, seq);
