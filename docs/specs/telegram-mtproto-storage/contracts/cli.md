# CLI Contracts（telegram mtproto）

> Kind: CLI（internal）
>
> Binary: `televybackup` (`crates/cli/`)

## 1) `televybackup telegram validate`

- 范围（Scope）: internal
- 变更（Change）: Modify

### 用法（Usage）

```text
televybackup telegram validate [--json]
```

### 行为（Behavior）

前置：

- 读取 `config.toml` 并校验 `telegram.*` 配置。

模式分支：

- 当 `telegram.mode = "botapi"`：
  - 行为保持现有：`getMe` + `getChat` 校验 token 与 chat 可用性。
- 当 `telegram.mode = "mtproto"`：
  - 校验本地 secrets presence（bot token / api_hash）：
    - 缺失则返回可操作错误（提示用户先设置对应 secrets）。
  - 校验 vault/secrets store 可用性：
    - vault key 仅存 Keychain 1 条（`televybackup.vault_key`），用于解密本地 secrets store（见
      `./file-formats.md`）。
  - 建立 MTProto 会话（必要时触发 bot 登录与 session 初始化/持久化）。
  - 校验对目标 `chat_id` 的访问能力（至少能发送与读取 document）。
  - 执行一次端到端回环验证：
    1) 上传一个测试对象（bytes 可为随机/固定，但必须可重复比对）。
    2) 以返回的 `object_id` 下载对象。
    3) 比对上传/下载内容一致。

约束：

- validate 过程中输出与日志必须脱敏，不得泄露 token/session/api_hash。
- validate 不得污染 `--events` NDJSON 输出契约（本命令不输出 events）。

### 输出（Output）

- `--json=false`（human，stdout）：
  - `mode=<botapi|mtproto>`
  - `chatId=<...>`
  - `botUsername=<...>`（若可获得）
  - `roundTripOk=<true|false>`（仅 mtproto）
  - `sampleObjectId=<...>`（仅 mtproto；用于排错；不包含 secrets）
- `--json=true`（json，stdout）：

示例（botapi）：

```json
{"mode":"botapi","botUsername":"...","chatId":"-100..."}
```

示例（mtproto）：

```json
{"mode":"mtproto","chatId":"-100...","roundTripOk":true,"sampleObjectId":"tgmtproto:v1:..."}
```

### 错误（Errors）

- `config.invalid`: 配置缺失/无效（例如 `telegram.chat_id` 为空）
- `telegram.unauthorized`: bot token 缺失或无效
- `telegram.mtproto.missing_api_hash`: mtproto 模式下缺失 `api_hash`
- `telegram.mtproto.session_invalid`: session 失效（建议清理 session 后重新 validate）
- `telegram.chat_not_found`: chat 不存在或 bot 无权限访问
- `telegram.unavailable`: 网络/Telegram 不可达（retryable）
- `telegram.roundtrip_failed`: 回环校验失败（需输出可排查信息但不得泄露 secrets）
- `secrets.vault_unavailable`: vault key/Keychain 不可用（需引导用户修复系统 Keychain 权限）
- `secrets.store_failed`: secrets store 不可读/不可解密/损坏（需引导用户重建或恢复 secrets store）

## 2) `televybackup secrets set-telegram-api-hash`

- 范围（Scope）: internal
- 变更（Change）: New

### 用法（Usage）

```text
televybackup secrets set-telegram-api-hash [--json]
```

### 输入（Input）

- 从 stdin 读取 `api_hash`（与 `set-telegram-bot-token` 一致）。

### 输出（Output）

- `--json=true`: `{"ok":true}`
- `--json=false`: `ok`

### 备注（Notes）

- 写入的 secrets store entry key 由 `telegram.mtproto.api_hash_key` 指定；缺省建议为
  `telegram.mtproto.api_hash`（见 `./config.md`）。
- 不在输出中回显输入内容。

## 3) `televybackup secrets clear-telegram-mtproto-session`

- 范围（Scope）: internal
- 变更（Change）: New

### 用法（Usage）

```text
televybackup secrets clear-telegram-mtproto-session [--json]
```

### 行为（Behavior）

- 从 secrets store 中删除 `telegram.mtproto.session_key` 对应 entry（若不存在视为成功）。

### 输出（Output）

- `--json=true`: `{"ok":true}`
- `--json=false`: `ok`

## 4) `televybackup secrets migrate-keychain`（一次性迁移：Keychain → secrets store）

- 范围（Scope）: internal
- 变更（Change）: New

### 用法（Usage）

```text
televybackup secrets migrate-keychain [--json]
```

### 行为（Behavior）

目的：把历史版本写入 Keychain 的 secrets（bot token / master key）迁移到本地加密 secrets store，并将
Keychain secrets 收敛为仅 1 条 vault key。

- 读取旧 Keychain items：
  - `telegram.bot_token_key` 对应的 item（例如默认 `telegram.bot_token`）
  - `televybackup.master_key`
- 写入 secrets store（entry key 同名）：
  - `telegram.bot_token_key`
  - `televybackup.master_key`
- 迁移成功后删除旧 Keychain items（不删除 vault key）：
  - 目标：避免 Keychain item 过多导致频繁授权弹窗

### 输出（Output）

- `--json=true`（json）示例：

```json
{"ok":true,"migrated":["telegram.bot_token","televybackup.master_key"],"deletedKeychainItems":["telegram.bot_token","televybackup.master_key"]}
```

- `--json=false`（human）：
  - `ok`

### 错误（Errors）

- `secrets.vault_unavailable`: Keychain/vault key 不可用
- `keychain.unavailable`: 运行环境不支持 Keychain（非 macOS）
- `secrets.store_failed`: secrets store 不可读/不可解密/写入失败
