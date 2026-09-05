# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## Testing

- 单元测试覆盖：
  - retry/backoff 的错误分类与重试边界（例如 timeout / flood wait）。
- 本地最小验证：
  - `cargo test -p mtproto-helper`


## Milestones

- [x] 为 `save_file_part` / `save_big_file_part` 实装 retry + backoff（含 flood wait 支持）。
- [x] upload 阶段增加 `upload_progress` heartbeat（无进度也输出）。
- [x] 补齐与 retry/heartbeat 相关的单元测试。

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/njr29:mtproto-upload-resilience/PLAN.md`.
