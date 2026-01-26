# CLI Contracts

本文件定义本计划新增的 CLI 接口：用于 macOS UI 获取“状态快照”（Overview/Dev 共用）。

## 1) `televybackup status get`

- Scope: internal
- Change: New
- Output: JSON（单对象）
- Exit codes:
  - `0`: success（即使没有任何 targets，也返回结构化空值）
  - `!=0`: failure（返回 `CliError` JSON；与现有 CLI 约定一致）

### Usage

```bash
televybackup --json status get
```

### Response schema

- `StatusSnapshot`：见 `contracts/events.md` 中 `status.snapshot` 的 payload（同一 schema，便于复用）。

### Errors

- `config.invalid`: 配置缺失或不可解析
- `status.unavailable`: 状态源不可用（例如 daemon 未运行且无法生成快照）

## 2) `televybackup status stream`

- Scope: internal
- Change: New
- Output: NDJSON（每行一个 JSON 对象）
- Semantics:
  - 每行都是完整快照（`type="status.snapshot"`），UI 可无状态渲染。
  - 选择 NDJSON 的原因：可逐行增量解析（无需等待/缓存完整 JSON 数组），天然适合长连接流；单行损坏也更易跳过/重连；并且便于用 `tail -f` / 日志管道做排障。
  - 运行中推荐频率：`5–10Hz`；静止态可降至 `1Hz`（由实现决定，但必须稳定）。
  - 必须包含 `generatedAt`（用于 stale 判定）。
  - 数据来源：从 daemon 落盘 `status.json` 读取；CLI 作为适配层对上层输出统一事件流（UI 不直读文件）。

### Usage

```bash
televybackup --json status stream
```

### Output lines

每行：

```json
{ "type": "status.snapshot", "...": "..." }
```

### Termination / reconnect

- 若 stream 退出：UI 侧应显示 stale，并尝试指数退避重连（实现策略在 impl 阶段确定）。

### Fallback behavior (no daemon snapshots)

- 若 `status.json` 不存在或不可读：`status get/stream` 会返回一个由 `settings v2` 合成的最小快照（`source.kind="cli"`）。
- 合成快照的 `targets[]` 仅包含配置态字段（`targetId/label/sourcePath/endpointId/enabled`），并将 `state="stale"` 以提示“缺少运行态状态源”。
