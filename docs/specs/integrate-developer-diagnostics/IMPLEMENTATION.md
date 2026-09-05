# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 测试计划（Test Plan）

- 必跑：`TELEVYBACKUP_APP_VARIANT=dev scripts/macos/build-app.sh`
- 建议：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all-features`


## 里程碑（Milestones）

- [x] 移除 Developer window 与 Settings 入口
- [x] Main window 增加 `History | Diagnostics` 分段 + Diagnostics 内容
- [x] 文档同步（architecture + IA + UI README）
- [x] 本地验证通过 + PR 创建 + CI checks 结果明确

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/7f9wg:integrate-developer-diagnostics/PLAN.md`.
