# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## Testing

- 自动化验证：
  - `cargo test -p televy_backup_core`
  - `cargo test -p televybackupd`
  - `cargo test -p televybackup`
  - `cargo test --manifest-path /Users/ivan/Projects/Ivan/televy-backup/crates/mtproto-helper/Cargo.toml`
- 现场验证：
  - 以 `Projects` 单目标运行 backup，并采样 30 分钟 status stream。
  - 统计是否 30 分钟内完成、以及 `>=1 MiB/s` 累计分钟数。


## Milestones

- [x] M1: core 上传侧自适应并发/节流控制器落地并接入主备份流程
- [x] M2: helper watchdog + send_message heartbeat 改造完成
- [x] M3: sqlite 锁竞争恢复（busy_timeout + 限定重试）落地
- [x] M4: 错误优先级修正 + 测试覆盖
- [x] M5: 本地自动化验证 + 30 分钟现场验收 + PR 结果收敛

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/7r6p4:background-backup-adaptive-throughput/PLAN.md`.
