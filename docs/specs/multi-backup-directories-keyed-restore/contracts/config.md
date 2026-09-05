# Config Contracts（multi backup targets + telegram endpoints）

> Kind: Config（internal）

## 1) `config.toml`：schema v2（frozen）

目标：表达“多个 backup target ↔ Telegram endpoint（bot token + chat_id）绑定关系”，并保持 secrets 不落盘。

### Top-level

- `version`: number
  - 必填：是
  - 取值：`2`
- `targets`: array（TOML `[[targets]]`）
- `telegram_endpoints`: array（TOML `[[telegram_endpoints]]`）
- 其他全局策略（沿用 v1，作为默认值）：
  - `schedule`（global default）
  - `retention`（global）
  - `chunking`（global）
  - `telegram`（global Telegram settings）

### `[schedule]`（global default schedule）

语义：作为每个 target 的默认 schedule；target 可通过 `[targets.schedule]` override（见下文）。

- `enabled`: bool
- `kind`: `"hourly" | "daily"`
- `hourly_minute`: number（0-59）
- `daily_at`: string（`HH:MM`）
- `timezone`: `"local"`（固定）

### `[[targets]]`（backup target）

每个 target 对应一个“备份目录条目”。

目录与 Bot 信息的关系（frozen）：

- 一个 `target` 必须绑定且仅绑定一个 `endpoint_id`
- 一个 `endpoint_id` 对应一组 Bot 信息（`bot_token_key + chat_id`），可被多个 target 复用（满足“多个目录可配置同一个 Bot”）

- `id`: string
  - 必填：是
  - 约束：稳定且不变；建议使用可读 slug（避免空格与特殊字符）
- `source_path`: string（绝对路径；UTF-8）
  - 必填：是
- `label`: string
  - 必填：否（缺省视为 `"manual"` 或空；最终由实现冻结）
- `endpoint_id`: string
  - 必填：是
  - 语义：引用 `[[telegram_endpoints]].id`
- `enabled`: bool
  - 必填：否
  - 默认：`true`

#### `[targets.schedule]`（per-target schedule override）

语义：每个 target 有独立 schedule；默认继承 global schedule，并可按字段 override。

- 该 table 可缺省：表示全量继承 `[schedule]`
- 该 table 内字段可缺省：表示对应字段继承 `[schedule]`

字段（均为可选；存在则覆盖同名 global 字段）：

- `enabled`: bool（允许显式关闭某个 target 的 schedule）
- `kind`: `"hourly" | "daily"`
- `hourly_minute`: number（0-59）
- `daily_at`: string（`HH:MM`）

### `[[telegram_endpoints]]`（Telegram storage endpoint）

每个 endpoint 表示一个 `(bot token, chat_id)` 组合；多个 target 可引用同一 endpoint。

- `id`: string
  - 必填：是
  - 约束：稳定且不变；用于 provider namespace（见下文）
- `mode`: string
  - 必填：是
  - 取值：`"mtproto"`（当前实现限定）
- `chat_id`: string
  - 必填：是
  - 说明：必须用 string 存储以避免 int 溢出/序列化差异
- `bot_token_key`: string
  - 必填：是
  - 说明：secrets store entry key；不得写 token 明文
- `rate_limit.max_concurrent_uploads`: number
- `rate_limit.min_delay_ms`: number

### 示例（仅示意）

```toml
version = 2

[schedule]
enabled = true
kind = "hourly"
hourly_minute = 0
daily_at = "02:00"
timezone = "local"

[[telegram_endpoints]]
id = "default"
mode = "mtproto"
chat_id = "-100123..."
bot_token_key = "telegram.bot_token.default"

[telegram_endpoints.rate_limit]
max_concurrent_uploads = 2
min_delay_ms = 250

[[targets]]
id = "aaa-bbb"
source_path = "/AAA/BBB"
label = "manual"
endpoint_id = "default"
enabled = true

[targets.schedule]
# 覆盖为 daily，其他未写字段继承 global
kind = "daily"
daily_at = "03:00"
```

## 2) v1 兼容与迁移策略（frozen）

v1（当前）形状（简化）：

- `sources[]`
- `telegram.*`（单一 endpoint）

兼容策略：

1. **读取兼容**：若 `version` 缺失则按 v1 解析，并在内存中映射为 v2：
   - 生成一个 endpoint：`id="default"`，字段从 `telegram.*` 映射
   - 为每个 `sources[i]` 生成一个 target：`source_path = sources[i]`，并生成稳定 `id`
2. **写回迁移**：当 UI/CLI 保存设置时，写回 v2 格式（`version=2`）。

`target.id` 生成规则（用于 v1 → v2 迁移，frozen）：

- `id = "src_" + blake3(source_path_utf8).to_hex()[0..8]`
- 约束：稳定、低冲突；避免在路径变更时产生不可预测差异

## 3) Provider namespace（frozen）

目的：避免多 endpoint 场景下 index/objects 混用导致的不可恢复。

- 规则：`provider = "telegram.mtproto/<endpoint_id>"`
  - `<endpoint_id>` 来自 config；不得包含 bot token 明文
  - 作为 SQLite 中 `provider` 字段的值（remote_index_* / chunk_objects）
- 约束：一旦 endpoint 投入使用（产生远端对象），`endpoint_id` 必须视为不可变（否则会导致 provider 断裂，影响 restore）。

## 4) Secrets（vault key + 本地加密 secrets store）

- Keychain：只存 1 条 vault key（`televybackup.vault_key`），用于解密本地 secrets store（`secrets.enc`）
- Telegram Bot token：secrets store entry key = `[[telegram_endpoints]].bot_token_key`
- Master key：secrets store entry key = `televybackup.master_key`（Base64, 32 bytes）

`config.toml` 中不得出现：

- bot token 明文
- master key 明文或金钥明文
