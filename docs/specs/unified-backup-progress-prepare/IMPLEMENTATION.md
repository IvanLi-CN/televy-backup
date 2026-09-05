# Implementation

## Current State

- Existing Spec status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 进度口径（NeedUploadConfirmed / UploadingCurrent / BackedUp / Scanned）已冻结。
- prepare 并行语义与失败语义已冻结。
- 目标 UI 位点已冻结（MainWindow 3 处 + Popover 1 处）。


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests:
  - core/daemon：新增字段透传与兼容。
  - cli：prepare 并行逻辑与 events 字段输出。
- Integration tests:
  - backup pipeline 在 prepare 并行下行为正确。

### Quality checks

- `cargo test`
- `cargo test -p televybackup-cli`
- `cargo test -p televybackupd`
- `scripts/macos/build-app.sh`


## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: 完成 core/daemon/cli 进度字段扩展与兼容透传
- [x] M2: 完成 Prepare 并行（CLI + daemon）
- [x] M3: 完成主窗口与浮窗统一进度组件替换
- [x] M4: 完成测试、构建验证与文档同步

## Migration State

- Legacy ID-prefixed directory normalized to this slug-only topic.
