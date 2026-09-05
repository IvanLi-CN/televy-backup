# Implementation

## Current State

- Existing Spec status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 范围、非目标、验收标准已冻结。
- UI 变更仅限 Popover 路径，且不改外部契约。
- 允许走快车道推进到 PR + checks + review-loop 收敛。


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- `scripts/macos/build-app.sh` 成功。
- `scripts/macos/swift-unit-tests.sh` 成功。
- 手工验证 A/B/C/D 场景（1 target、2 targets、多 targets、empty/stale/disconnected）。

### Quality checks

- 变更文件通过仓库现有构建检查；不引入 `--no-verify` 提交。


## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: 接入 Targets 真实内容高度测量并回传布局层。
- [x] M2: Popover 高度改为实测驱动并增加抖动抑制。
- [x] M3: Running 摘要单行截断并完成场景回归验证。
- [x] M4: 创建 PR 并完成 checks + review-loop 收敛。

## Migration State

- Legacy ID-prefixed directory normalized to this slug-only topic.
