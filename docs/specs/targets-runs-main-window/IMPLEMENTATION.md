# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 非功能性验收 / 质量门槛（Quality Gates）

- UI 不应在主线程做大规模日志扫描（需要后台线程/增量扫描策略）。
- 日志解析失败不应影响主界面可用性（失败时降级为“无记录/部分记录”并提示）。
- 不把运行/交付依赖挂在 `docs/plan/` 下。


## 实现里程碑（Milestones）

- [x] M1: macOS 主界面窗口（Targets 列表/详情路由）与浮窗入口调整
- [x] M2: Restore UI（空目录校验 + 触发 restore latest + 任务反馈）
- [x] M3: Verify UI（触发 verify latest + 任务反馈）
- [x] M4: CLI `verify latest` + `restore/verify` run log 补齐 `target_id`
- [x] M5: 执行记录摘要聚合展示（按 target 分组 + 最近 N 条）与端到端验收

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/kaa5e:targets-runs-main-window/PLAN.md`.
