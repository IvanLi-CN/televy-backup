# Implementation

## Current State

- Legacy plan status: `待实现`.
- Canonical implementation state: `planned`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认 local index 的文件布局：采用 **方案 B（本地按 endpoint 拆分多文件）**：
  - 每个 endpoint 一个 sqlite（路径约定见 `./contracts/file-formats.md`）
  - backup/restore/verify 都按 `target.endpoint_id` 选择对应 db


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit：chat_id 唯一性校验；provider 过滤/拆库的正确性
- Integration：两 endpoints 交替备份/恢复，不发生跨 endpoint 复用与污染


## 实现里程碑（Milestones）

- [x] M1: 冻结 local index layout（B：本地按 endpoint 拆分索引库）
- [ ] M2: 实现 remote index endpoint-scoped（过滤/拆库）
- [ ] M3: Settings 校验：chat_id 全局唯一
- [ ] M4: 测试覆盖（多 endpoints）
- [ ] M5: 文档同步（architecture + related plans）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/r6ceq:endpoint-scoped-index-chat-uniqueness/PLAN.md`.
