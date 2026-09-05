# IPC Contracts（native macOS app ↔ `televybackup` CLI）

> Kind: IPC（internal）
>
> Transport: local process (`televybackup`), argv + stdin/stdout/stderr
>
> Error shape: JSON on stdout in `--json` mode + non-zero exit code

## Common flags

- `--json`: 输出机器可读 JSON（单个对象）到 stdout
- `--events`: 在长任务中输出 NDJSON events 到 stdout（见 `events.md`）

## Common types

### Error JSON (`CliError`)

- `code`: string（稳定枚举，便于前端判断）
- `message`: string（面向用户的错误信息）
- `details`: object（便于诊断；不得包含敏感信息；无信息时为 `{}`）
- `retryable`: boolean（是否应重试）

示例：

```json
{ "code": "telegram.rate_limited", "message": "Too many requests", "retryable": true }
```

## Commands (argv)

### `ping`

**Request**

- `value`: string

**Response**

- `string`（例如 `pong: <value>`）

**Errors**

- None

### `backup run`

**Request**

- `sourcePath`: string（本地绝对路径）
- `label`: string（人类可读标签；允许为空字符串）

**Response**

- `snapshotId`: string（创建后即可返回；或在任务完成时填充，见实现决策）

**Errors**

- `config.invalid`
- `source.not_found`
- `source.not_readable`

### `restore run`

**Request**

- `snapshotId`: string
- `targetPath`: string（本地绝对路径；必须不存在或为空目录）

**Response**

- `ok`: boolean

**Errors**

- `snapshot.not_found`
- `target.not_empty`
- `target.not_writable`

### `verify run`

**Request**

- `snapshotId`: string

**Response**

- `ok`: boolean

### `settings get`

**Request**: none

**Response**

- `settings`: object（见 `file-formats.md` 的配置口径；不得包含任何 secret 明文）
- `secrets`: object
  - `telegramBotTokenPresent`: boolean
  - `masterKeyPresent`: boolean

### `settings set`

**Request**

- `settings`: object（完整设置）

**Response**

- `settings`: object（持久化后的最终值）

**Errors**

- `config.invalid`

### `secrets set-telegram-bot-token`

用于写入/更新 Keychain 中的 Telegram bot token。

**Request**

- 从 stdin 读取 token（UTF-8 文本，去除首尾空白）

**Response**: `{ "ok": true }`

**Errors**: `keychain.unavailable` / `keychain.write_failed`

### `secrets init-master-key`

生成 32-byte master key 并写入 Keychain（Base64 编码）。

**Response**: `{ "ok": true }`

**Errors**: `keychain.unavailable` / `keychain.write_failed`

### `telegram validate`

用于验证 Bot API token 与目标私聊 `chat_id` 是否可用（例如能否 `getMe`、能否 `getChat`）。

**Request**: none

**Response**

- `botUsername`: string
- `chatId`: string

**Errors**

- `telegram.unauthorized`
- `telegram.chat_not_found`
- `telegram.forbidden`
- `telegram.rate_limited`（retryable）
