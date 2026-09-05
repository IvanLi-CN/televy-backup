# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## Testing

- 单元测试：
  - error 分类（索引下载：暂时性 Telegram vs 永久缺失）。
  - FloodWait 解析新格式。
- 集成/最小验证：
  - `cargo test --all-features`
  - `cd crates/mtproto-helper && cargo test`


## Milestones

- [x] core：远端索引下载错误分类修复 + 测试覆盖
- [x] core：扫描阶段容错（忽略瞬态 NotFound）+ 单测（非 flaky）
- [x] core：backup collect 批量写入 `chunk_objects`，避免 sqlite pool 超时 + 测试覆盖
- [x] helper：下载 retry+断点续传、FloodWait 解析增强、send_message 重试 + 单测
- [x] core+helper：helper init 协议与 config 扩展（`min_delay_ms`/`max_concurrent_uploads`）并打通 daemon/cli 创建点
- [x] daemon：Vault key 缓存可失效 + crypto 失败自愈（带护栏）+ 最小验证

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/7bq4a:prod-backup-verify-stability/PLAN.md`.
