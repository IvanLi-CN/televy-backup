-- TelevyBackup MVP schema (see docs/plan/0001:telegram-backup-mvp/contracts/db.md)

CREATE TABLE IF NOT EXISTS schema_migrations (
  version INTEGER PRIMARY KEY,
  applied_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
  snapshot_id TEXT PRIMARY KEY,
  created_at TEXT NOT NULL,
  source_path TEXT NOT NULL,
  label TEXT NOT NULL,
  base_snapshot_id TEXT NULL
);

CREATE INDEX IF NOT EXISTS idx_snapshots_created_at
  ON snapshots(created_at);

CREATE TABLE IF NOT EXISTS files (
  file_id TEXT PRIMARY KEY,
  snapshot_id TEXT NOT NULL REFERENCES snapshots(snapshot_id),
  path TEXT NOT NULL,
  size INTEGER NOT NULL,
  mtime_ms INTEGER NOT NULL,
  mode INTEGER NOT NULL,
  kind TEXT NOT NULL,
  UNIQUE (snapshot_id, path)
);

CREATE TABLE IF NOT EXISTS chunks (
  chunk_hash TEXT PRIMARY KEY,
  size INTEGER NOT NULL,
  hash_alg TEXT NOT NULL,
  enc_alg TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS chunk_objects (
  chunk_hash TEXT NOT NULL REFERENCES chunks(chunk_hash),
  provider TEXT NOT NULL,
  object_id TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY (provider, object_id),
  UNIQUE (provider, chunk_hash)
);

CREATE TABLE IF NOT EXISTS remote_indexes (
  snapshot_id TEXT PRIMARY KEY REFERENCES snapshots(snapshot_id),
  provider TEXT NOT NULL,
  manifest_object_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS remote_index_parts (
  snapshot_id TEXT NOT NULL REFERENCES snapshots(snapshot_id),
  part_no INTEGER NOT NULL,
  provider TEXT NOT NULL,
  object_id TEXT NOT NULL,
  size INTEGER NOT NULL,
  hash TEXT NOT NULL,
  PRIMARY KEY (snapshot_id, part_no)
);

CREATE TABLE IF NOT EXISTS file_chunks (
  file_id TEXT NOT NULL REFERENCES files(file_id),
  seq INTEGER NOT NULL,
  chunk_hash TEXT NOT NULL REFERENCES chunks(chunk_hash),
  offset INTEGER NOT NULL,
  len INTEGER NOT NULL,
  PRIMARY KEY (file_id, seq)
);

CREATE TABLE IF NOT EXISTS tasks (
  task_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  state TEXT NOT NULL,
  created_at TEXT NOT NULL,
  started_at TEXT NULL,
  finished_at TEXT NULL,
  snapshot_id TEXT NULL,
  error_code TEXT NULL,
  error_message TEXT NULL
);

INSERT OR IGNORE INTO schema_migrations(version, applied_at)
  VALUES (1, strftime('%Y-%m-%dT%H:%M:%fZ','now'));

