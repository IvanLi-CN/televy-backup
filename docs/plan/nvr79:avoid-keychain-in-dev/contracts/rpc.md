# RPC 契约：Daemon control IPC（#nvr79）

## 目标

提供一个 daemon-only 的控制面 IPC，让 CLI 与 macOS app 获取所需能力，而不直接访问：

- Keychain
- `vault.key`
- `secrets.enc`

该 IPC 只暴露“presence/状态/写入动作”，不返回 vault key 明文。

## Transport

- Kind: Unix domain socket
- Socket path: `<TELEVYBACKUP_DATA_DIR>/ipc/control.sock`
  - 说明：当前 status IPC 使用 `<TELEVYBACKUP_DATA_DIR>/ipc/status.sock`；控制面必须独立 socket，避免协议混用。
- Encoding: JSON Lines（每行一条 JSON，`\n` 分隔；UTF-8）
- Timeouts: client-side 默认 500ms（可按操作类型调整）

## Auth / Scope

- Scope: internal
- Auth: 依赖本机 unix socket 文件权限（同用户进程可访问；daemon 创建时确保目录与文件权限合理）

## Message envelope

### Request

```json
{
  "type": "control.request",
  "id": "uuid-or-rand",
  "method": "vault.status",
  "params": {}
}
```

### Response

```json
{
  "type": "control.response",
  "id": "uuid-or-rand",
  "ok": true,
  "result": {}
}
```

Error:

```json
{
  "type": "control.response",
  "id": "uuid-or-rand",
  "ok": false,
  "error": {
    "code": "control.unavailable",
    "message": "daemon not reachable",
    "retryable": true,
    "details": {}
  }
}
```

## Methods

（以下方法是本计划建议的最小集；实现时可在不破坏兼容的前提下增量扩展）

### 1) `vault.status`

用途：给 UI/CLI 展示当前 vault backend 状态与风险提示依据（不泄漏密钥）。

- Params: `{}`  
- Result:
  - `backend`: `"keychain"` | `"file"`
  - `keyPresent`: boolean（当前 backend 下 vault key 是否可用）
  - `keychainDisabled`: boolean（是否 `TELEVYBACKUP_DISABLE_KEYCHAIN=1`）
  - `vaultKeyFilePath`: string | null（backend=file 时返回）

### 2) `vault.ensure`

用途：保证 vault key 可用；在 `keychainDisabled=true` 时允许自动创建 `vault.key`（无交互中断）。

- Params: `{}`
- Result: 同 `vault.status`

### 3) `secrets.presence`

用途：替代当前 UI 通过 CLI 读 `secrets.enc` 的 presence 逻辑；由 daemon 负责解密后返回“是否存在”。

- Params:
  - `endpointId`: string | null（可选，若为 null 则返回所有 endpoint 的 presence map）
- Result（建议形状，与当前 `settings get --with-secrets` 的输出对齐）：
  - `masterKeyPresent`: boolean
  - `telegramMtprotoApiHashPresent`: boolean
  - `telegramBotTokenPresentByEndpoint`: `{ "<endpointId>": boolean, ... }`
  - `telegramMtprotoSessionPresentByEndpoint`: `{ "<endpointId>": boolean, ... }`

### 4) `secrets.setTelegramBotToken`

- Params:
  - `endpointId`: string
  - `token`: string
- Result: `{ "ok": true }`

### 5) `secrets.setTelegramApiHash`

- Params:
  - `apiHash`: string
- Result: `{ "ok": true }`

### 6) `secrets.clearTelegramMtprotoSession`

- Params:
  - `endpointId`: string
- Result: `{ "ok": true }`

## Compatibility rules

- Server 必须忽略未知字段（forward-compatible）。
- Client 必须能处理新增字段（backward-compatible）。
- `method` 新增不破坏旧 client；旧 server 对未知 method 返回 `control.method_not_found`（retryable=false）。

## Errors

- `control.unavailable`: daemon 不可达/IPC 不可用（retryable=true）
- `control.timeout`: 超时（retryable=true）
- `control.invalid_request`: 请求格式错误（retryable=false）
- `control.method_not_found`: method 不存在（retryable=false）
- `secrets.store_failed`: secrets store 解密/读写失败（retryable=false 或按错误类型）
- `secrets.vault_unavailable`: vault key 不可用（按 `contracts/config.md`）
