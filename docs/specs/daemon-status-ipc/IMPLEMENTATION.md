# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

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


## 实现里程碑（Milestones）

- [x] M1: daemon status IPC server（socket + NDJSON 输出 + 限频）
- [x] M2: CLI 默认改为 IPC，并实现 fallback
- [x] M3: 测试与文档更新（契约/断线/时延）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0011:daemon-status-ipc/PLAN.md`.
