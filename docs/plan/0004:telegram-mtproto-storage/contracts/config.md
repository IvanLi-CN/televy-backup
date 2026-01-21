# Config Contracts（telegram mtproto mode）

> Kind: Config（internal）
>
> Format: `config.toml`（v1 schema；本计划不引入 v2 targets/endpoints 结构）

## 1) `[telegram]`（新增 mtproto 模式）

- 范围（Scope）: internal
- 变更（Change）: Modify

### 字段

- `telegram.mode`: string
  - 必填：是
  - 取值：`"botapi" | "mtproto"`
  - 默认：`"botapi"`
- `telegram.chat_id`: string
  - 必填：是
  - 语义：目标对话（与 bot 私聊/或指定 chat）定位；与既有语义一致
- `telegram.bot_token_key`: string
  - 必填：是
  - 语义：secrets store entry key；用于读取 bot token（token 明文不得落盘）
  - 默认：`"telegram.bot_token"`（与既有默认一致；但底层存储从 Keychain 迁移到 secrets store）
- `telegram.rate_limit.max_concurrent_uploads`: number
- `telegram.rate_limit.min_delay_ms`: number
  - 语义：上传/下载的并发与节流上限（实现阶段决定是否对 download 单独拆字段；若拆分需先在此契约中固化）

### 新增：`[telegram.mtproto]`

当且仅当 `telegram.mode = "mtproto"` 时生效。

- `telegram.mtproto.api_id`: number
  - 必填：是（仅 mtproto 模式）
  - 语义：Telegram App `api_id`
  - 备注：`api_id` 本身不是 secret，可落盘
- `telegram.mtproto.api_hash_key`: string
  - 必填：是（仅 mtproto 模式）
  - 语义：secrets store entry key；用于读取 Telegram App `api_hash`（不得落盘）
  - 默认：`"telegram.mtproto.api_hash"`
- `telegram.mtproto.session_key`: string
  - 必填：是（仅 mtproto 模式；首次 validate/login 后会写入对应 secret）
  - 语义：secrets store entry key；用于读取/写入 MTProto session（不透明字节；由实现决定编码，建议 base64）
  - 默认：`"telegram.mtproto.session"`

## 2) Secrets（vault key + 本地加密 secrets store）

`config.toml` 中不得出现：

- bot token 明文
- MTProto session 明文
- `api_hash` 明文

### Keychain（只存 1 条 vault key）

- 仅存放一个 vault key：`televybackup.vault_key`（随机 32-byte；建议 Base64 存储）
- vault key 用途：解密本地 `secrets.enc`（见 `./file-formats.md`）
- 目标：避免 Keychain item 过多导致频繁授权弹窗

### 本地 secrets store（加密文件）

- 真实 secrets（bot token / master key / api_hash / mtproto session）写入本地加密 secrets store。
- `telegram.*_key` 字段指向该 store 内的 entry key（不是 Keychain item key）。
- 具体文件位置与格式见：`./file-formats.md`。

#### “session” 是什么（术语澄清）

- 本计划中的 “MTProto session” 指：MTProto 客户端的**持久化连接/鉴权状态**（例如 auth key 等），用于
  重启后复用登录状态；它不是用户账号密码，但仍属于敏感数据，必须加密存放并默认脱敏。

## 3) 兼容性与迁移（Compatibility / migration）

- `telegram.mode = "botapi"`：忽略 `[telegram.mtproto]`（即使存在）；行为保持与现有一致。
- `telegram.mode = "mtproto"`：
  - 缺失 `bot_token` / `api_hash` / `api_id` 时必须给出可操作的错误提示（例如提示先执行
    `telegram validate` 或对应 secrets 设置命令）。
  - `session` 缺失时可由 `telegram validate` 自动初始化并写回（仍需 vault key 可用）。
  - `chat_id` 语义与 botapi 模式保持一致。
