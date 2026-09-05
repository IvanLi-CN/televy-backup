# Implementation

## Current State

- Existing Spec status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 质量门槛（Quality Gates）

- `bash ./.github/scripts/test-release-scripts.sh`
- `cargo test --all-features`
- `cd crates/mtproto-helper && cargo test`
- `bash scripts/macos/swift-unit-tests.sh`


## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: 新增 `PR Label Gate` workflow 与标签校验脚本
- [x] M2: 拆分 `CI (PR)` / `CI (main)`，并保留现有测试矩阵
- [x] M3: 新增 `release-intent.sh`，支持 merge commit -> PR -> labels 决策
- [x] M4: 改造 `compute-version.sh` 为 semver bump 驱动
- [x] M5: 新增 `Release` workflow，支持 stable/rc/skip 与手动 backfill
- [x] M6: 补充脚本合同测试与 README 发布规则说明
- [x] M7: 补充 `docs/quality-gates.md`，声明 required checks 与 bootstrap waiver

## Migration State

- Legacy ID-prefixed directory normalized to this slug-only topic.
