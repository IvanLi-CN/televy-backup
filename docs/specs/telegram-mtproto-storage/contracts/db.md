# DB Contracts（SQLite index / object_id encoding）

> Kind: DB（internal）
>
> Engine: SQLite
>
> Migration: 本计划优先采用“无需 schema 变更”的兼容策略（字段仍为 TEXT；仅扩展编码形状）

本计划的 DB 变更目标：让索引能够持久化 MTProto 引用，并在引用过期时仍可“恢复定位”，从而完成大对象的下载与恢复。

## 1) 影响范围（Affected fields）

- `chunk_objects.object_id`
- `remote_index_parts.object_id`
- `remote_indexes.manifest_object_id`

## 2) `object_id` 编码（新增 `tgmtproto:`）

- 范围（Scope）: internal
- 变更（Change）: Modify

### Change（旧 → 新）

旧（#0001/#0002，Bot API）：

- **remote index objects**（`remote_index_parts.object_id` / `remote_indexes.manifest_object_id`）：
  - `file_id`（Bot API `file_id`；无前缀）
- **chunk objects**（`chunk_objects.object_id`）：
  - `file_id`（历史遗留；无前缀）
  - `tgfile:<file_id>`（direct）
  - `tgpack:<file_id>@<offset>+<len>`（pack slice）

新（#0004，增量）：

在上述基础上新增一种“storage object id”形状（由 `Storage.upload_document` 返回）：

- `tgmtproto:v1:<payload_b64url>`

并将其用于各字段：

- **remote index objects**（不使用 `tgfile:` 包裹）：
  - `remote_index_parts.object_id = <storage_object_id>`
  - `remote_indexes.manifest_object_id = <storage_object_id>`
- **chunk objects**（沿用既有 `tgfile:` / `tgpack:` 包裹规则）：
  - direct：`tgfile:<storage_object_id>`（例如 `tgfile:tgmtproto:v1:...`）
  - pack slice：`tgpack:<storage_object_id>@<offset>+<len>`（例如 `tgpack:tgmtproto:v1:...@...+...`）

其中：

- `payload_b64url` 为 Base64URL（RFC 4648 URL-safe；不包含 `+` `/`；建议无 padding `=`）
- payload 为 UTF-8 JSON（便于版本化与扩展字段；字段新增需保持向后兼容）

### Payload schema（v1）

```json
{
  "peer": "<chat_id>",
  "msgId": "<message_id>",
  "docId": "<document_id>",
  "accessHash": "<access_hash>"
}
```

字段说明：

- `peer`：目标对话标识（与配置中的 `telegram.chat_id` 一致；string 存储）。
- `msgId`：包含该 document 的消息定位信息（用于“刷新引用/重新获取 file_reference”）。
- `docId` / `accessHash`：document 的稳定标识（用于下载定位与基本一致性校验）。

下载与引用刷新策略（frozen）：

- `tgmtproto:v1` payload **不存** `file_reference`（避免过期导致 `object_id` 失效）。
- 下载前必须通过 `peer+msgId` 拉取消息/文档元数据，并使用其中的最新 `file_reference` 执行下载。

### 与 pack slice 的兼容性约束

pack slice 编码为 `tgpack:<pack_object_id>@<offset>+<len>`，其中分隔符为 `@` 与 `+`。

为避免歧义：

- `tgmtproto:v1:<payload_b64url>` 形状中不得出现 `@` 与 `+`（Base64URL 可满足该约束）。
- 若未来引入其他 `object_id` 前缀，必须保证其不会包含 `@` 与 `+`，或在 pack slice 中对 `pack_object_id` 做额外转义/编码（需要另立契约变更）。

## 3) 读取兼容与错误提示（Compatibility）

本计划不要求 `telegram.mtproto` 兼容读取历史 Bot API 备份数据。

对于未知前缀（例如 `tgmtproto:v2:` 或未来扩展）：

- 必须返回明确错误码与可操作提示（例如提示升级版本或重新备份生成新的引用）。

### Provider mismatch（必须可操作）

- 当运行时选择 `telegram.mode=mtproto`，但目标 snapshot/objects 的 `provider` 为 `telegram.botapi`：
  - 必须返回明确错误与修复建议：切回 `telegram.mode=botapi` 以恢复旧 snapshot，或重新执行一次备份以生成 mtproto snapshot。

## 4) Migration notes（迁移说明）

- Schema delta：无（字段类型不变）
- Rollout steps:
  1) 上线 `tgmtproto:v1:` 解析与 `telegram.mtproto` 存储实现
  2) 开启 mtproto 写入（在 `telegram.mode=mtproto` 下写入 `tgmtproto:v1:` 与 `tgpack:tgmtproto:v1:`）
- 回滚策略：
  - 切回 `telegram.mode=botapi`：继续可用 Bot API 路径；mtproto snapshot 需要 mtproto 模式才能恢复（不做自动迁移）
- Backfill / data migration：
  - 不做自动迁移；若需要把历史 Bot API 引用迁移到 MTProto，另立计划实现工具链
