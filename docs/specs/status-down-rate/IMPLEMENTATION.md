# Implementation

## Current State

- Legacy plan status: `未检查`.
- Canonical implementation state: `in-progress`.

## Migrated Delivery Notes

## 测试 / 验证

- `cargo test --workspace`
- 本地手工：运行 macOS app（prod variant）并触发 restore/verify/index sync，观察 NETWORK Down 的刷新与稳定性。


## 里程碑（Milestones）

- [x] 进度上报支持 streaming：remote index 下载过程中持续更新 `bytes_downloaded`
- [x] status 速率来源稳定：daemon 提供 1s window down rate；CLI `status stream` 不再覆盖 daemon 的 down rate
- [x] 手工验收：在 macOS UI 中观察 NETWORK Down（2Hz 刷新，±10%；无明显不可能尖峰；与 Activity Monitor 趋势正相关）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/mycnc:status-down-rate/PLAN.md`.
