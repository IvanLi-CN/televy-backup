# Event Contracts（`televybackup` stdout NDJSON）

> Kind: Event（internal）
>
> Producer: `televybackup`（Rust CLI）
>
> Consumers: native macOS app
>
> Delivery semantics: at-least-once（消费者应按 `taskId` 幂等更新 UI）

## Events

All events are printed as one JSON object per line (NDJSON).

Common fields:

- `type`: string（event type）
- `taskId`: string

### `task.state`

**Payload**

- `kind`: `"backup" | "restore" | "verify"`
- `state`: `"queued" | "running" | "succeeded" | "failed" | "cancelled"`
- `error`?: `{ code: string, message: string }`

### `task.progress`

**Payload**

- `phase`: string（见 core 的 `phase` 约定）
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
