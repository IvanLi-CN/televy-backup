# Event Contracts（Tauri events）

> Kind: Event（internal）
>
> Producer: Tauri backend（Rust）
>
> Consumers: Web UI
>
> Delivery semantics: at-least-once（前端应按 `taskId` 幂等更新 UI）

## Events

### `task:state`

**Payload**

- `taskId`: string
- `kind`: `"backup" | "restore" | "verify"`
- `state`: `"queued" | "running" | "succeeded" | "failed" | "cancelled"`
- `error`?: `{ code: string, message: string }`

### `task:progress`

**Payload**

- `taskId`: string
- `phase`: string（见 RPC `phase` 约定）
- `filesTotal`?: number
- `filesDone`?: number
- `chunksTotal`?: number
- `chunksDone`?: number
- `bytesRead`?: number
- `bytesUploaded`?: number
- `bytesDeduped`?: number
- `throughputBytesPerSec`?: number
- `etaSeconds`?: number

## Compatibility rules

- 允许新增字段（前端忽略未知字段）。
- 禁止删除/重命名字段；如需变更，必须新增字段并走弃用周期。

