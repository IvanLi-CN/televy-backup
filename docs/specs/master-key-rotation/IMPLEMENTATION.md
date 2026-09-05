# Implementation

## Current State

- Legacy plan status: `待实现`.
- Canonical implementation state: `planned`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 已冻结 per-endpoint 索引库布局（`#r6ceq`：Option B）。
- 已确定 rotation state 的持久化位置与形状（见 `./contracts/file-formats.md`）。
- 已确定二次确认的具体交互：typed phrase（输入 `ROTATE`），覆盖 start/commit（cancel/pause 不需要二次确认）。


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit：状态机转移；rotation state 持久化；双轨索引库切换的原子性
- Integration：断点续跑（kill/restart）；pause/resume/cancel；commit 切换正确


## 实现里程碑（Milestones）

- [ ] M1: Rotation state 规格 + 持久化
- [ ] M2: CLI 接口（start/pause/resume/cancel/status）
- [ ] M3: 双轨索引库（next 写入 + commit 原子切换）
- [ ] M4: 远端 catalog 双轨（un-pinned 更新 + commit pin）
- [ ] M5: 测试与文档同步

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/4fexy:master-key-rotation/PLAN.md`.
