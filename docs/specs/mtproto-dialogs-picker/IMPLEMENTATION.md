# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 非功能性验收 / 质量门槛（Quality Gates）

- dialogs 列表需有整体超时/错误提示，避免 UI 永久等待。
- 不把运行/交付依赖挂在 `docs/plan/` 下。


## 实现里程碑（Milestones）

- [x] M1: MTProto helper 支持 chat-less init + wait-chat 稳定输出（含超时）
- [x] M2: CLI `telegram wait-chat` 可用（chat_id 允许为空）
- [x] M3: macOS Settings 增加 “Listen…” picker 并能写回 chat_id
- [x] M4: 端到端验证（CLI + UI）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0013:mtproto-dialogs-picker/PLAN.md`.
