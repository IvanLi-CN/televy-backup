# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 测试 / 验证

- `cargo test`（覆盖 compare 的 match/mismatch/extra/missing/hash mismatch 等单元测试）。
- `./scripts/macos/build-app.sh`（确保 Swift 编译通过）。
- UI snapshot（可选）：展示 compare 行为与冲突选项（无需依赖真实网络即可截图）。


## 里程碑（Milestones）

- [x] 核心：实现 local-vs-remote snapshot content compare（remote index DB + 本地 bytes）
- [x] CLI：暴露 `settings import-bundle --compare-folder` JSON 接口供 UI 调用
- [x] UI：inspect 后自动 compare；mismatch 才要求冲突选择；按选项执行 restore/backup
- [x] 文档：澄清 compare/冲突处理语义，强调不依赖本地 index DB

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/w2k9p:import-bundle-rebind-remote-compare/PLAN.md`.
