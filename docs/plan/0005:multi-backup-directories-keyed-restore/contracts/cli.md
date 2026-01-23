# CLI Contracts（settings v2 + recovery）

> Kind: CLI（internal）
>
> Transport: local process (`televybackup`) argv + stdin/stdout/stderr

本计划涉及对 CLI/IPC 的扩展，用于：

- 管理多个 Telegram endpoint 的 secrets（bot token）
- 导入/导出 master key（金钥）
- 支持跨设备恢复（无需旧 SQLite 的 `manifest_object_id`）

## Common flags（existing）

- `--json`: 输出单个 JSON object 到 stdout
- `--events`: 在长任务中输出 NDJSON events 到 stdout

## 1) `settings get`

### Response（`--json`）

必须返回 v2 settings 结构（不含 secrets 明文）：

- `settings.version = 2`
- `settings.schedule`（global default）
- `settings.targets[]`
- `settings.telegram_endpoints[]`
- `secrets`：
  - `masterKeyPresent`: bool
  - `telegramBotTokenPresentByEndpoint`: object（key 为 endpoint_id，value 为 bool）
  - `telegramMtprotoApiHashPresent`: bool
  - `telegramMtprotoSessionPresentByEndpoint`: object（key 为 endpoint_id，value 为 bool）

示例（仅示意）：

```json
{
  "settings": {
    "version": 2,
    "schedule": { "enabled": true, "kind": "hourly", "hourly_minute": 0, "daily_at": "02:00", "timezone": "local" },
    "targets": [{ "id": "aaa-bbb", "source_path": "/AAA/BBB", "endpoint_id": "default", "enabled": true }],
    "telegram_endpoints": [{ "id": "default", "mode": "mtproto", "chat_id": "-100123...", "bot_token_key": "telegram.bot_token.default" }]
  },
  "secrets": {
    "masterKeyPresent": true,
    "telegramBotTokenPresentByEndpoint": { "default": true },
    "telegramMtprotoApiHashPresent": true,
    "telegramMtprotoSessionPresentByEndpoint": { "default": true }
  }
}
```

## 2) `telegram validate`（multi-endpoint）

用于验证指定 endpoint（token + chat_id）是否可用，以及是否具备读取 pinned message 的能力。

- `televybackup telegram validate --endpoint-id <endpoint_id>`

## 3) `settings set`

- Request：从 stdin 读取 TOML（v2）
- Behavior：校验并写入 `config.toml`

## 4) `secrets set-telegram-bot-token`（multi-endpoint）

目标：能为指定 endpoint 写入/更新 bot token（secrets store）。

约束：token 从 stdin 输入（避免 argv 泄露）。

接口（frozen）：

- `televybackup secrets set-telegram-bot-token --endpoint-id <endpoint_id>`
  - token 从 stdin 读取（UTF-8 文本；trim 首尾空白）
  - 使用该 endpoint 的 `bot_token_key` 写入 secrets store
  - 不允许通过 argv 传 token

## 5) “金钥”导出/导入（master key portability）

### `secrets export-master-key`（new）

用途：从 secrets store 读取 master key，并以“金钥字符串”导出（用于迁移到新设备）。

安全要求：

- 必须要求显式确认参数（例如 `--i-understand`），避免误触把 secret 打到终端日志。
- `--json` 模式应输出结构化字段，便于 UI 消费但不默认展示。

金钥字符串格式（frozen）：

- `TBK1:<base64url_no_pad>`
  - `<base64url_no_pad>` 为 32 bytes master key 的 base64url（不含 `=` padding）
  - 仅用于人类传递；等价于 secrets store 中的 `televybackup.master_key`（32 bytes）

示例（仅示意）：

```json
{ "goldKey": "TBK1:base64url...", "format": "tbk1" }
```

### `secrets import-master-key`（new）

用途：把“金钥字符串”从 stdin 导入并写入 secrets store。

约束：

- 默认不得覆盖已存在 master key（除非显式 `--force`）。
- 必须校验格式与长度（32 bytes）。

## 6) 跨设备恢复入口（new/modify）

目标：用户不需要手动输入 `snapshot_id` 与 `manifest_object_id` 也能恢复。

接口（frozen）：

- `televybackup restore latest --target-id <target_id> --target <path>`
- `televybackup restore latest --source-path <source_path> --target <path>`

解析规则：

- `--target-id`：直接定位 target
- `--source-path`：在 settings.targets 中查找 `source_path` 匹配的 target；若多条匹配（例如不同 endpoint），必须报错提示改用 `--target-id`

依赖：

- 能从 endpoint 的 bootstrap/catalog 解析出 latest 的 `snapshot_id + manifest_object_id`
- 错误码稳定（例如 `bootstrap.missing` / `bootstrap.forbidden` / `bootstrap.invalid`）
