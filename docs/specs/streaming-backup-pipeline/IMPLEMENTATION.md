# Implementation

## Current State

- Existing Spec status: `部分完成（4/5）`.
- Canonical implementation state: `in-progress`.

## Migrated Delivery Notes

## 测试与验证（Testing）

- `cargo test -p televy-backup-core`
- `cargo test -p televybackup-cli`
- `cargo test -p televybackupd`
- `scripts/macos/swift-unit-tests.sh`
- `cargo test -p televy_backup_core --test pack_uploads`


## 里程碑（Milestones）

- [x] M1: core 主循环 scan+upload 并行化
- [x] M2: 阶段与进度字段语义对齐
- [x] M3: 单元测试覆盖并通过
- [x] M4: retention 清理性能优化（批处理 + 索引调整 + 回归测试）
- [ ] M5: macOS UI 验证与截图确认

## Migration State

- Legacy ID-prefixed directory normalized to this slug-only topic.
