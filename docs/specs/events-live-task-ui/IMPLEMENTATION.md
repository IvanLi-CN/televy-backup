# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 非功能性验收 / 质量门槛（Quality Gates）

- 至少补齐对应的单元测试（CLI events 输出行为、UI 解析逻辑的最小覆盖）。
- 不引入长驻进程/服务启动变更。


## 里程碑（Milestones）

- [x] M1: CLI `--events` 事件输出统一封装并强制 flush（含 failed 事件）
- [x] M2: macOS GUI 完整消费 `task.progress` 字段并展示 restore/verify 进度
- [x] M3: macOS GUI 引入 active task 状态机，running/finish 及时可见，端到端验收通过

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/fwwqp:events-live-task-ui/PLAN.md`.
