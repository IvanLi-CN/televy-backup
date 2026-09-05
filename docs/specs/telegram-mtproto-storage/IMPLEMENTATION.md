# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

None


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests:
  - `tgmtproto:` object_id 的解析/序列化与版本化（包含“不得包含 `@`/`+` 等 pack slice 分隔符”的约束）。
  - 分片下载断点续传的正确性（使用 mock storage 或可控 stub；不依赖真实 Telegram）。
  - 错误分类与重试策略（可重试/不可重试）与脱敏规则（token/session 不出现在日志/事件中）。
- Integration tests:
  - 在不访问真实 Telegram 的前提下跑一次最小 restore/verify 流程（使用 in-memory storage 或 stub）。
  - 手工验收：`telegram validate` + 真实大对象 restore（作为 release 前 checklist，不自动化进 CI）。

### Quality checks

- `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`（不引入新工具）。


## 实现里程碑（Milestones）

- [x] M1: 冻结具体 MTProto crate + session 持久化方案（并在契约中固化）
- [x] M2: `telegram.mtproto` 基础连通（bot 登录 + upload_document + 小对象 download_document）
- [x] M3: 大对象下载（分片/续传/重试 + 节流/并发控制 + 低内存峰值）
- [x] M4: DB 口径与 `object_id` 形状落地（`tgmtproto:v1:` + provider 不匹配错误提示）
- [x] M5: CLI validate + GUI 状态展示 + 文档与测试补齐
- [x] M6: MTProto-only：移除 Bot API 全链路（core/cli/daemon/gui/docs）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0004:telegram-mtproto-storage/PLAN.md`.
