# Implementation

## Current State

- Existing Spec status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- helper: `cargo test --manifest-path crates/mtproto-helper/Cargo.toml -- --nocapture`
- core: `cargo test -p televy_backup_core telegram_mtproto -- --nocapture`
- macOS app build: `TELEVYBACKUP_APP_VARIANT=prod TELEVYBACKUP_CODESIGN_IDENTITY=- ./scripts/macos/build-app.sh`

### Quality checks

- Rust: `cargo fmt --all -- --check`


## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: helper 协议新增 `shutdown`，并在 EOF / shutdown 下统一退出 sender pool
- [x] M2: core helper wrapper 改为 graceful shutdown + kill fallback，并覆盖 drop / respawn
- [x] M3: daemon idle cache clear 增加结构化 teardown 日志
- [x] M4: helper/core 补进程级回归测试并通过定向验证
- [x] M5: macOS run history 改为磁盘历史回填 + 大日志头尾索引，避免重启后空白误报

## Migration State

- Legacy ID-prefixed directory normalized to this slug-only topic.
