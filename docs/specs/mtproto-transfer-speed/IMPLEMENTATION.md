# Implementation

## Current State

- Existing Spec status: `实现完成，待 PR 合并、Release 与真机验收`.
- Canonical implementation state: `in-progress`.

## Migrated Delivery Notes

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: helper FloodWait parser + core transient 分类/检测的单测覆盖。
- Integration tests: 无新增（以现有备份/上传集成测试为准）。

### Quality checks

- Rust: `cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all-features`
- helper: `cd crates/mtproto-helper && cargo test`
- Swift（如 UI 变更触发）：`scripts/macos/swift-unit-tests.sh`


## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: helper part size 提升到 512KiB（upload+download），并通过单测/真实运行验证无 part size 错误
- [x] M2: helper FloodWait parser 支持 `FLOOD_PREMIUM_WAIT`，并加入全局冷却 + progress 心跳
- [x] M3: core 增加 `FLOOD_PREMIUM_WAIT` 的 transient/降档检测与单测
- [x] M4: macOS UI 增加 “Rate limit (advanced)” 控件并通过 swift 单测（如适用）
- [x] M5: CI 增加 helper tests 步骤并全绿
- [x] M6: core 引入 helper pool 并实现多 helper session 隔离（仅 primary helper 更新持久化 session）
- [ ] M7: 将 direct、pack、index-part 置于一个共享的非阻塞有界调度器；发布后以同机 Projects NDJSON 甘特图验证实际 file-part RPC 并发与全局上限

## Migration State

- Legacy ID-prefixed directory normalized to this slug-only topic.
