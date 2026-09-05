# Implementation

## Current State

- Existing Spec status: `部分完成（3/4）`.
- Canonical implementation state: `in-progress`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 规格已明确“裁剪规则（按 source latest）”与恢复/同步不变式
- 单测覆盖多 source、多 snapshot 的裁剪正确性


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: `cargo test -p televy-backup-core`
- Full workspace: `cargo test --all-features`

### Quality checks

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`


## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: 在 `upload_index` 中实现 compact export DB（按 source latest 裁剪 files/file_chunks）
- [x] M2: 添加单测覆盖裁剪规则
- [x] M3: backup 成功后自动 compact 本地 endpoint index DB，并添加单测
- [ ] M4: 真机验证：观察 index upload bytes 与耗时下降，且 index_sync + 下一次 backup 正常

## Migration State

- Legacy ID-prefixed directory normalized to this slug-only topic.
