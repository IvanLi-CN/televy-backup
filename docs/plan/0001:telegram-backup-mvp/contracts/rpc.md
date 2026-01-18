# RPC Contracts（Tauri commands / invoke）

> Kind: RPC（internal）
>
> Transport: Tauri `@tauri-apps/api/core` `invoke()`
>
> Error shape: 统一返回 `Result<T, RpcError>`

## Common types

### RpcError

- `code`: string（稳定枚举，便于前端判断）
- `message`: string（面向用户的错误信息）
- `details`: object（便于诊断；不得包含敏感信息；无信息时为 `{}`）
- `retryable`: boolean（是否应重试）

示例：

```json
{ "code": "telegram.rate_limited", "message": "Too many requests", "retryable": true }
```

## Commands

### `ping`（已存在）

**Request**

- `value`: string

**Response**

- `string`（例如 `pong: <value>`）

**Errors**

- None

### `backup_start`

**Request**

- `sourcePath`: string（本地绝对路径）
- `label`: string（人类可读标签；允许为空字符串）

**Response**

- `taskId`: string
- `snapshotId`: string（创建后即可返回；或在任务完成时填充，见实现决策）

**Errors**

- `config.invalid`
- `source.not_found`
- `source.not_readable`
- `task.already_running`

### `backup_status`

**Request**

- `taskId`: string

**Response**

- `state`: `"queued" | "running" | "succeeded" | "failed" | "cancelled"`
- `phase`: `"scan" | "chunk" | "upload" | "index" | "finalize" | "idle"`
- `progress`: object（见 events 的 payload 形状；无进度时返回 `{}`）

**Errors**

- `task.not_found`

### `backup_cancel`

**Request**

- `taskId`: string

**Response**

- `ok`: boolean

**Errors**

- `task.not_found`

### `restore_start`

**Request**

- `snapshotId`: string
- `targetPath`: string（本地绝对路径；必须不存在或为空目录）

**Response**

- `taskId`: string

**Errors**

- `snapshot.not_found`
- `target.not_empty`
- `target.not_writable`

### `restore_status`

同 `backup_status`（phase 不同：`download`/`reassemble`/`verify`）。

### `restore_cancel`

同 `backup_cancel`。

### `verify_start`

**Request**

- `snapshotId`: string

**Response**

- `taskId`: string

### `verify_status`

同 `backup_status`（phase：`index`/`chunks`/`reassemble`）。

### `settings_get`

**Request**: none

**Response**

- `settings`: object（见 `file-formats.md` 的配置口径；不得包含任何 secret 明文）
- `secrets`: object
  - `telegramBotTokenPresent`: boolean
  - `masterKeyPresent`: boolean

### `settings_set`

**Request**

- `settings`: object（完整设置）
- `secrets`: object（仅用于写入/更新 Keychain，不会被原样回显）
  - `telegramBotToken`: string | null（为 null 表示不变更）
  - `rotateMasterKey`: boolean（固定 false；MVP 不做轮换）

**Response**

- `settings`: object（持久化后的最终值）

**Errors**

- `config.invalid`
- `keychain.unavailable`
- `keychain.write_failed`

### `telegram_validate`

用于验证 Bot API token 与目标私聊 `chat_id` 是否可用（例如能否 `getMe`、能否发消息/上传文件到目标 chat）。

**Request**: none

**Response**

- `botUsername`: string
- `chatId`: string

**Errors**

- `telegram.unauthorized`
- `telegram.chat_not_found`
- `telegram.forbidden`
- `telegram.rate_limited`（retryable）

### `stats_get`

**Request**: none

**Response**

- `snapshotsTotal`: number
- `chunksTotal`: number
- `bytesUploadedTotal`: number
- `bytesDedupedTotal`: number
