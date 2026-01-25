# daemon 状态 IPC：替换 file-based 状态源（#0011）

## 状态

- Status: 待实现
- Created: 2026-01-24
- Last: 2026-01-25

## 背景 / 问题陈述

- 计划 #0010 的实时状态面板在“无 socket IPC”的前提下，采用 daemon 落盘 `status.json` 作为状态真源；该方案实现简单，但存在 IO 频率、原子读写、以及“读取延迟/抖动”的结构性上限。
- 需要一个独立计划，将“状态真源”从 file-based 升级为 daemon 暴露本地 IPC（Unix domain socket），由 CLI 转发给 GUI，从而降低延迟与 IO 开销，并提升可观测性一致性。

## 目标 / 非目标

### Goals

- 新增 daemon→CLI 的本地 IPC（Unix domain socket），提供 `StatusSnapshot` 的流式输出。
- 将 `televybackup status stream/get` 的默认数据源切换到 IPC（保留 file-based 作为 fallback，见兼容策略）。
- 明确断线、重连、stale、以及 daemon 不在时的错误语义（对 GUI 可测试）。

### Non-goals

- 不在本计划内重做 #0010 的 UI 设计（本计划只替换“数据源”）。
- 不在本计划内对 daemon 的调度/业务逻辑做重构（只增加状态投影与 IPC 输出）。

## 范围（Scope）

### In scope

- daemon：新增 status IPC server（socket lifecycle、并发、限频）。
- CLI：`status get/stream` 优先走 IPC；并提供清晰 fallback。
- 契约：RPC/事件/CLI 的可实现可测试文档。

### Out of scope

- macOS UI 改造（除非为接入新错误码/状态字段所需的最小改动；具体放在 impl 阶段评估）。

## 需求（Requirements）

### MUST

- IPC transport：Unix domain socket（stream）。
- IPC payload：NDJSON（每行一个 `StatusSnapshot`；一行完整快照；便于增量解析与调试）。
- socket path：
  - 默认：`$TELEVYBACKUP_DATA_DIR/ipc/status.sock`
- 首条快照：客户端连接后 `≤ 500ms` 内必须发出第一条（用于快速判定 live）。
- cadence：
  - 运行中：`5–10Hz`（建议上限 10Hz）
  - 静止态：`1Hz`
- 不输出 secrets：不得包含 bot token/master key/api_hash/session 等任何敏感字段。
- CLI 行为：
  - `televybackup --json status stream` 默认连接 IPC 并输出 `status.snapshot` NDJSON。
  - IPC 不可用时：根据策略 fallback（见下文 Compatibility）。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| daemon status IPC | RPC | internal | New | ./contracts/rpc.md | daemon | CLI | socket NDJSON 输出 |
| `televybackup status get/stream` | CLI | internal | Modify | ./contracts/cli.md | CLI | macOS UI | 默认数据源由 file→IPC |
| `status.snapshot` | Event | internal | Modify | ./contracts/events.md | CLI | macOS UI | schema 保持向后兼容（additive） |
| `status.json`（fallback） | File format | internal | Modify | ./contracts/file-formats.md | daemon | CLI | fallback only |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/rpc.md](./contracts/rpc.md)
- [contracts/cli.md](./contracts/cli.md)
- [contracts/events.md](./contracts/events.md)
- [contracts/file-formats.md](./contracts/file-formats.md)

## 兼容性与迁移（Compatibility / migration）

- Default：CLI 优先连接 IPC；成功则以 IPC 输出为准。
- Fallback（当 socket 不存在/连接失败）：
  - 读取 `status.json`（若存在且可解析）并继续对 UI 输出 `status.snapshot`
  - 若两者都不可用：返回 `status.unavailable`
- 迁移窗口：实现初期允许同时保留 `status.json` 写入，便于回滚与排障；待稳定后再评估是否降频/移除文件写入（另行决策）。

## 验收标准（Acceptance Criteria）

- Given daemon 正在运行并暴露 socket，
  When CLI 执行 `televybackup --json status stream`，
  Then 输出 NDJSON `status.snapshot` 且首条在 `≤ 500ms` 内出现，并达到 cadence 目标。
- Given daemon 未运行或 socket 不存在，
  When CLI 执行 `status stream/get`，
  Then 按 fallback 策略输出快照或返回 `status.unavailable`，且错误语义稳定可测试。
- Given 输出快照，
  When 检查字段，
  Then 不包含任何 secrets。

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认 socket path 与权限模型（user-level daemon + user-level GUI/CLI）。
- 已确认 fallback 策略（file-based 继续保留的时长与降级口径）。
- 已确认 `StatusSnapshot` schema 的版本演进策略（additive-only + bump 条件）。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit: IPC framing（NDJSON）、断线/重连、首条时延、限频逻辑。
- Contract: schema 兼容（旧 UI 可容错新字段）。

### Performance

- daemon 输出限频（≤ 10Hz），CPU/内存稳定。
- 相比 file-based：减少频繁文件写入与 GUI/CLI 的读抖动。

## 文档更新（Docs to Update）

- `docs/plan/0010:status-popover-dashboard/contracts/cli.md`: 如数据源策略对 UI 有可见影响，补充说明（实现阶段同步）。

## 实现里程碑（Milestones）

- [ ] M1: daemon status IPC server（socket + NDJSON 输出 + 限频）
- [ ] M2: CLI 默认改为 IPC，并实现 fallback
- [ ] M3: 测试与文档更新（契约/断线/时延）
