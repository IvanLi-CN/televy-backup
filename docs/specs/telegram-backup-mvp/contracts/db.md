# DB Contracts（SQLite index）

> Kind: DB（internal）
>
> Engine: SQLite
>
> Migration: 使用 `schema_migrations` 版本表 + 逐步迁移（SQL migrations 随代码版本一起发布，策略为 forward-only）

## Schema（MVP）

> 注意：此处为“可实现的最小形状”，允许增量补字段/索引，但不应在未更新契约的情况下做破坏性变更。

### `schema_migrations`

- `version` INTEGER PRIMARY KEY
- `applied_at` TEXT NOT NULL（ISO8601）

### `snapshots`

- `snapshot_id` TEXT PRIMARY KEY
- `created_at` TEXT NOT NULL（ISO8601）
- `source_path` TEXT NOT NULL（绝对路径）
- `label` TEXT NOT NULL（允许为空字符串）
- `base_snapshot_id` TEXT NULL（指向上一快照；可为空）

索引：
- `idx_snapshots_created_at` on (`created_at`)

### `files`

- `file_id` TEXT PRIMARY KEY
- `snapshot_id` TEXT NOT NULL REFERENCES `snapshots`(`snapshot_id`)
- `path` TEXT NOT NULL（相对 `source_path` 的路径）
- `size` INTEGER NOT NULL
- `mtime_ms` INTEGER NOT NULL（Unix epoch milliseconds）
- `mode` INTEGER NOT NULL（POSIX mode；未知时为 0）
- `kind` TEXT NOT NULL（`file|dir|symlink`）

约束：
- UNIQUE (`snapshot_id`, `path`)

### `chunks`

- `chunk_hash` TEXT PRIMARY KEY（hex）
- `size` INTEGER NOT NULL
- `hash_alg` TEXT NOT NULL（例如 `blake3`）
- `enc_alg` TEXT NOT NULL（例如 `xchacha20poly1305`）
- `created_at` TEXT NOT NULL

### `chunk_objects`

用于记录 chunk 在 Telegram 的存储引用（允许支持多 provider）。

- `chunk_hash` TEXT NOT NULL REFERENCES `chunks`(`chunk_hash`)
- `provider` TEXT NOT NULL（`telegram.botapi` / `telegram.mtproto` / ...）
- `object_id` TEXT NOT NULL（稳定 id；可为复合字段的序列化）
- `created_at` TEXT NOT NULL

约束：
- PRIMARY KEY (`provider`, `object_id`)
- UNIQUE (`provider`, `chunk_hash`)

#### `object_id` 编码（#0002 增量）

`object_id` 允许两种形状（字符串编码，便于兼容与迁移）：

1) 独立对象（兼容 #0001）：

- `tgfile:<file_id>`
- 迁移期内也允许裸 `<file_id>`（无前缀）

2) pack 内切片（#0002 新增）：

- `tgpack:<file_id>@<offset>+<len>`

字段语义：

- `<file_id>`：存储后端返回的对象 id（Telegram Bot API 为 `file_id`；测试存储可能为 `mem:...`）
- `<offset>`：十进制字节偏移（从 pack 文件起始算起；指向该 chunk 的“加密 blob”）
- `<len>`：十进制字节长度（该 chunk 的“加密 blob”长度）

### `remote_indexes`

记录每次备份完成后上传的“索引 manifest”（加密）的远端引用。

- `snapshot_id` TEXT PRIMARY KEY REFERENCES `snapshots`(`snapshot_id`)
- `provider` TEXT NOT NULL（MVP 固定 `telegram.botapi`）
- `manifest_object_id` TEXT NOT NULL（Bot API `file_id`）
- `created_at` TEXT NOT NULL

### `remote_index_parts`

记录索引分片在远端的引用（用于快速恢复；内容也会在 manifest 内重复描述，DB 记录用于本地加速与一致性检查）。

- `snapshot_id` TEXT NOT NULL REFERENCES `snapshots`(`snapshot_id`)
- `part_no` INTEGER NOT NULL
- `provider` TEXT NOT NULL（MVP 固定 `telegram.botapi`）
- `object_id` TEXT NOT NULL（Bot API `file_id`）
- `size` INTEGER NOT NULL
- `hash` TEXT NOT NULL（hex；与 `hash_alg` 一致）

约束：
- PRIMARY KEY (`snapshot_id`, `part_no`)

### `file_chunks`

- `file_id` TEXT NOT NULL REFERENCES `files`(`file_id`)
- `seq` INTEGER NOT NULL（从 0 开始）
- `chunk_hash` TEXT NOT NULL REFERENCES `chunks`(`chunk_hash`)
- `offset` INTEGER NOT NULL（在文件中的偏移；必须保留）
- `len` INTEGER NOT NULL

约束：
- PRIMARY KEY (`file_id`, `seq`)

### `tasks`

- `task_id` TEXT PRIMARY KEY
- `kind` TEXT NOT NULL（`backup|restore|verify`）
- `state` TEXT NOT NULL（`queued|running|succeeded|failed|cancelled`）
- `created_at` TEXT NOT NULL
- `started_at` TEXT NULL
- `finished_at` TEXT NULL
- `snapshot_id` TEXT NULL（backup/verify 对应的 snapshot）
- `error_code` TEXT NULL
- `error_message` TEXT NULL

## Query patterns（MVP）

- 获取最新 N 个 snapshots（按 `created_at` desc）。
- 获取 snapshot 的文件清单与 chunk 序列（restore/verify）。
- 根据 `chunk_hash` 查 Telegram 对象引用（download/复用）。
