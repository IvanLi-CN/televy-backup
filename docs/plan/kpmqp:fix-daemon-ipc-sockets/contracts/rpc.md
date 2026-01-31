# RPC Contracts（IPC sockets: control + vault）

> Kind: RPC（internal）
>
> Transport: local unix domain sockets（under `TELEVYBACKUP_DATA_DIR/ipc/`）

本计划不改变现有 RPC 协议形状；这里只把“可用性与不变约束”写成可验证契约，供实现与测试使用。

## 1) Socket paths（固定）

- Control IPC: `TELEVYBACKUP_DATA_DIR/ipc/control.sock`
- Vault IPC: `TELEVYBACKUP_DATA_DIR/ipc/vault.sock`

约束：

- 当 `televybackupd` 正常运行时，上述 socket 必须存在且可连接。
- 若因残留 socket 文件导致 bind 失败：daemon 启动时应进行安全清理并重试（不改变权限边界）。

## 2) Control IPC（`control.request`）

### Envelope

- Request JSON：
  - `type = "control.request"`
  - `id: string`（非空）
  - `method: string`（非空）
  - `params: object`（optional）
- Response JSON：
  - `ok: bool`
  - `result: any | null`
  - `error: { code, message, retryable, details? } | null`

### Method: `secrets.presence`

- Purpose: 返回 secrets presence（不返回明文）。
- Params: `{ "endpointId": string | null }`
- Success result（shape，camelCase）：
  - `masterKeyPresent: bool`
  - `telegramMtprotoApiHashPresent: bool`
  - `telegramBotTokenPresentByEndpoint: object<string,bool>`
  - `telegramMtprotoSessionPresentByEndpoint: object<string,bool>`

Errors（示例；不新增/不改 code）：

- `control.unavailable`：IPC 不可用/无法连接
- `control.timeout`：读写超时
- `control.invalid_request`：请求 envelope/params 无效
- `control.method_not_found`：method 不存在
- `control.failed`：daemon 内部失败（`details` 里包含 `daemonCode/daemonDetails`）

## 3) Vault IPC

### Request types

（沿用现有约定，不新增字段）

- `vault.get_or_create`
- `keychain.get`
- `keychain.delete`

### Invariants

- 当 `televybackupd` 正常运行时，Vault IPC 必须可连接。
- 对于需要 Keychain 的操作：允许因用户交互导致耗时，但不得造成“socket 不监听/不可连接”。
