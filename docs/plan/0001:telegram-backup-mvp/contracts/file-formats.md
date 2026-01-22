# File Formats Contracts（local config / cache / index packaging）

> Kind: File format（internal）

## 1) 配置文件（Config）

### 位置

- 配置目录：`$TELEVYBACKUP_CONFIG_DIR`（未设置时使用 `~/Library/Application Support/TelevyBackup/`）
- 配置文件：`config.toml`

### `config.toml`（schema v2）

当前实现使用 **settings v2**（`version = 2`），支持多备份目录与多 Telegram endpoint：

- `version`: number（固定 `2`）
- `schedule`：全局调度默认值（targets 默认继承，可按 target 覆盖）
- `retention`：保留策略（只影响本地索引可见性，不做远端 chunk GC）
- `chunking`：
  - `min_bytes`
  - `avg_bytes`
  - `max_bytes`
- `telegram`：
  - `mode`: `"mtproto"`（MTProto-only）
  - `mtproto.api_id`
  - `mtproto.api_hash_key`（指向本地 secrets store 的 key 名；不得写明文）
- `telegram_endpoints[]`（TOML `[[telegram_endpoints]]`）：
  - `id`: string（稳定标识；用于 provider namespace）
  - `mode`: `"mtproto"`
  - `chat_id`: string（必须用字符串存储以避免 int 溢出/序列化差异）
  - `bot_token_key`: string（secrets store entry key；不得写 token 明文）
  - `mtproto.session_key`: string（secrets store entry key；不得写 session 明文）
  - `rate_limit.*`
- `targets[]`（TOML `[[targets]]`）：
  - `id`: string（稳定标识）
  - `source_path`: string（绝对路径；一个目录一个 target）
  - `label`: string
  - `endpoint_id`: string（引用 `telegram_endpoints[].id`）
  - `enabled`: boolean
  - `schedule`（可选 override；字段可部分覆盖全局）

兼容性：

- v1（无 `version` 的旧 shape）会在读取时迁移到 v2（内存映射）；保存时写回 v2。

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

### Secrets（Keychain + 本地加密 secrets store）

敏感信息不进 `config.toml`。

- Keychain（macOS）：只存 vault key（`televybackup.vault_key`，Base64 32 bytes）
- 本地 secrets store：`$TELEVYBACKUP_CONFIG_DIR/secrets.enc`
  - Telegram bot token：entry key = `[[telegram_endpoints]].bot_token_key`
  - MTProto session：entry key = `[[telegram_endpoints]].mtproto.session_key`
  - MTProto API hash：entry key = `telegram.mtproto.api_hash`（默认；由 `telegram.mtproto.api_hash_key` 指定）
  - Master key：entry key = `televybackup.master_key`（Base64 32 bytes）

## 2) 索引（SQLite）

- 索引文件：`index.sqlite`（位于 `$TELEVYBACKUP_DATA_DIR/index/`；未设置时同配置目录）
- 备份时的索引上传策略（MVP）：
  - 策略：上传 `index.sqlite` 的压缩+加密文件，然后再做**分片上传**（固定 part size），并上传一个加密的 `index-manifest` 用于恢复时重组

> 索引文件采取分片上传：避免单个索引对象过大，并让失败重试的粒度更小。

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
    { "no": 0, "size": 33554432, "hash": "hex...", "object_id": "tgmtproto:v1:..." }
  ]
}
```

说明：

- manifest **本身也加密上传**；其文件名中包含 `snapshot_id`，用于在 Telegram 历史中定位并下载。
- `object_id` 为远端存储对象的标识（当前实现为 `tgmtproto:v1:...`）。
- `index_part_bytes` 固定 32MiB（MVP 不提供可配置项；并与 pack 的目标大小对齐）。

## 3) 本地缓存（MVP 不实现）

本版本不实现本地缓存：不创建任何 `$APP_CACHE_DIR/chunks/*` 文件。

## 4) 兼容性规则

- 配置：新增字段向后兼容；删除/重命名字段需提供迁移逻辑。
- 索引：schema 版本必须可迁移；升级策略固定为“只前进（forward-only）”，不提供回滚迁移；变更必须同步更新 `db.md`。

## 5) Chunk 体积约束（Telegram）

Telegram 发送文件存在体积约束，会影响 chunk 的 `max_bytes`：

- chunking 的默认值按“安全留余量”的思路配置，并允许通过 `chunking.*` 调整。

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
  - pack header（#0002）：`televy.pack.header.v1`（固定；用于绑定 header 的 framing）

## 7) Pack 对象（#0002）

当“待上传对象”数量或体积超过阈值时，会启用 pack：把多个 chunk 的加密 blob 聚合为更少的上传对象，以降低上传调用次数。

- 远端存储对象：仍通过 Telegram `sendDocument` 上传一个 document
- pack 文件格式：见 `docs/plan/0002:small-object-packing/contracts/file-formats.md`

## 8) Remote bootstrap/catalog（#0005）

为支持“新设备无旧 SQLite 也能恢复”，每个 endpoint 维护一份加密的 bootstrap/catalog：

- 明文：JSON（UTF-8）
- 加密：沿用 framing（`encrypt_framed` / `decrypt_framed`）
  - key：master key（`televybackup.master_key`）
  - aad：固定 `televy.bootstrap.catalog.v1`
- 发现方式：使用 chat 的 pinned message 作为 root pointer（指向最新的 catalog document）

明文 JSON（示意）：

```json
{
  "version": 1,
  "updated_at": "2026-01-20T00:00:00Z",
  "targets": [
    {
      "target_id": "t_...",
      "source_path": "/AAA/BBB",
      "label": "manual",
      "latest": {
        "snapshot_id": "snp_...",
        "manifest_object_id": "tgmtproto:v1:..."
      }
    }
  ]
}
```
