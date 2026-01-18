# File Formats Contracts（local config / cache / index packaging）

> Kind: File format（internal）

## 1) 配置文件（Config）

### 位置

- 配置目录：`$APP_CONFIG_DIR/`（由 Tauri `appConfigDir` 决定）
- 配置文件：`config.toml`

### `config.toml`（MVP shape）

- `sources[]`：要备份的源目录列表（绝对路径）
- `schedule`：调度策略（hourly/daily + 时间点；由 `brew services` 管理的后台进程触发，App 不需要常驻）
- `retention`：保留策略（只影响本地索引可见性，不做远端 chunk GC）
- `chunking`：
  - `min_bytes`
  - `avg_bytes`
  - `max_bytes`
- `telegram`：
  - `mode`: `"botapi"`（已冻结）
  - `chat_id`: string（目标私聊 chat id；必须用字符串存储以避免 int 溢出/序列化差异）
  - `bot_token_key`: string（Keychain item key；不得写 token 明文）
  - `rate_limit`：
    - `max_concurrent_uploads`: number
    - `min_delay_ms`: number

### `schedule`（frozen）

- `enabled`: boolean
- `kind`: `"hourly" | "daily"`
- `hourly_minute`: number（0-59；`kind="hourly"` 时使用）
- `daily_at`: string（`HH:MM` 24 小时制；`kind="daily"` 时使用）
- `timezone`: `"local"`（固定）

语义：

- `enabled=false`：不自动触发；仍允许手动触发。
- `kind="hourly"`：每小时在 `hourly_minute` 触发一次。
- `kind="daily"`：每天本地时区 `daily_at` 触发一次。

### `retention`（frozen）

- `keep_last_snapshots`: number（>= 1）

语义：

- 备份成功后，若快照数超过 `keep_last_snapshots`，则从本地索引中删除更早的快照记录与其文件记录；不删除远端已上传的数据对象。

### Keychain（Secrets）

本计划约定：敏感信息只存 Keychain，不进 `config.toml`。

- `telegram.bot_token`：存为 Keychain item（key 为 `telegram.bot_token_key` 对应值）
- `crypto.master_key`：存为 Keychain item（固定 key，例如 `televybackup.master_key`；具体命名实现前再固化）
 - `crypto.master_key`：存为 Keychain item（固定 key：`televybackup.master_key`）

## 2) 索引（SQLite）

- 索引文件：`index.sqlite`（位于 `$APP_DATA_DIR/index/`）
- 备份时的索引上传策略（MVP）：
  - 策略：上传 `index.sqlite` 的压缩+加密文件，然后再做**分片上传**（每片 ≤ Bot API 限制），并上传一个加密的 `index-manifest` 用于恢复时重组

> Bot API 通过 `multipart/form-data` 上传“其他文件”最大 50MB，因此索引文件采取分片上传：只要求每个 part 在限制内（需要考虑加密/封装开销）。

### Index packaging（MVP, frozen）

1. 生成 `index.sqlite`
2. 压缩：得到 `index-<snapshot_id>.sqlite.zst`
3. 加密：得到 `index-<snapshot_id>.sqlite.zst.enc`
4. 分片：按固定 `index_part_bytes` 切分为 N 片：
   - `index-<snapshot_id>.sqlite.zst.enc.part-000000.bin`
   - ...
5. 上传：
   - 上传全部 `part-*`（每个 part 作为一个 Telegram `document`）
   - 上传 `index-<snapshot_id>.manifest.json.enc`（加密；用于描述 parts 顺序、大小、hash、以及各 part 的远端引用）

#### `index-manifest.json`（明文形状，实际上传为加密文件）

```json
{
  "version": 1,
  "snapshot_id": "snp_...",
  "hash_alg": "blake3",
  "enc_alg": "xchacha20poly1305",
  "compression": "zstd",
  "parts": [
    { "no": 0, "size": 33554432, "hash": "hex...", "object_id": "telegram_file_id" }
  ]
}
```

说明：

- manifest **本身也加密上传**；其文件名中包含 `snapshot_id`，用于在 Telegram 历史中定位并下载。
- `object_id` 为 Bot API 返回的 `file_id`（用于下载该 part）。
- `index_part_bytes` 固定 32MiB（MVP 不提供可配置项；确保每个 part 在 Bot API 限制内并留余量）。

## 3) 本地缓存（MVP 不实现）

本版本不实现本地缓存：不创建任何 `$APP_CACHE_DIR/chunks/*` 文件。

## 4) 兼容性规则

- 配置：新增字段向后兼容；删除/重命名字段需提供迁移逻辑。
- 索引：schema 版本必须可迁移；升级策略固定为“只前进（forward-only）”，不提供回滚迁移；变更必须同步更新 `db.md`。

## 5) Chunk 体积约束（Telegram Bot API）

Bot API 发送文件的约束会影响 chunk 的 `max_bytes`：

- `multipart/form-data` 上传：照片最大 10MB，其他文件最大 50MB
- `file_id` 复用发送：无限制（不作为“新增 chunk 上传”的主路径）

## 6) 加密封装（Encryption framing, frozen）

所有需要上传到 Telegram 的二进制对象（chunk blobs、索引 parts、索引 manifest）使用统一的二进制封装：

- `version`: 1 byte（固定 `0x01`）
- `nonce`: 24 bytes（随机）
- `ciphertext_and_tag`: 剩余全部字节（AEAD 输出）

算法：

- `enc_alg`: `xchacha20poly1305`
- Associated Data（AD）：
  - chunk blob：`chunk_hash`（hex 字符串的 UTF-8 bytes）
  - index part：`snapshot_id + ":" + part_no`（UTF-8 bytes）
  - index manifest：`snapshot_id`（UTF-8 bytes）
