# 文件格式（File formats）

> Kind: File format（internal）

本计划引入的文件格式契约：pack（远端对象）。

## 1) Pack 对象（pack file）（`pack-<pack_id>.bin`）

- 范围（Scope）: internal
- 变更（Change）: New
- 编码（Encoding）: binary

### 语义（Semantics）

- pack 是“上传对象”的聚合单位：一个 pack 内包含多个 chunk 的**加密 blob**以及一个**加密 header**。
- pack 的目标是减少 Bot API 调用次数：大量小 chunk 被合并为少量 pack 上传。

### Schema（结构）

pack 二进制布局（顺序拼接）：

1. `blob_1 || blob_2 || ... || blob_n`：每个 `blob_i` 是一个 chunk 的加密封装（沿用 #0001 已冻结的 chunk 加密 framing）
2. `pack_header.enc`：一个“加密封装”的 header（同样沿用统一 framing），其明文为 JSON（见下）
3. `pack_header_len_le_u32`：4 字节 little-endian 整数，表示 `pack_header.enc` 的字节长度

`pack_header.json`（明文形状；实际写入为加密文件）：

```json
{
  "version": 1,
  "hash_alg": "blake3",
  "enc_alg": "xchacha20poly1305",
  "entries": [
    { "chunk_hash": "hex...", "offset": 0, "len": 12345 }
  ]
}
```

说明：

- `offset/len` 指向 pack 文件内对应 `blob_i` 的位置（从文件起始算起）。
- `pack_header.enc` 的 Associated Data（AD）建议使用 `pack_id`（UTF-8 bytes），以防止 header 被置换。

### 兼容性与迁移（Compatibility / migration）

- `version` 递增时必须保持向后兼容读取（至少能识别旧版本并给出明确错误）。
- 允许未来在 `entries[]` 新增字段（向后兼容）；删除/重命名需迁移策略。

## 2) Pack 体积约束（Size constraints）

- `pack_target_bytes`（soft target）: `32MiB`
- `pack_max_bytes`（hard max）: `49MiB`（= `50MiB - 1MiB`，避免顶到 Bot API 上限）

说明：

- 上述字节数约束以“最终上传的 pack 文件大小”（包含 header 与 `pack_header_len_le_u32`）为准。
- 允许 pack 超过 `pack_target_bytes`，但任何情况下不得超过 `pack_max_bytes`。
