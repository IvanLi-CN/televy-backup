# Implementation

## Current State

- Existing Spec status: `部分完成（8/9）`.
- Canonical implementation state: `in-progress`.

## Migrated Delivery Notes

## 质量门槛（Quality Gates）

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features`


## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: Spec 与 docs/specs/README.md 索引落地
- [x] M2: bootstrap v1 扩展（endpointLatest）+ 单测
- [x] M3: endpoint DB upload/download（export 不含 files/file_chunks）+ endpoint_state
- [x] M4: snapshot filemap DB 生成/上传（remote_indexes 指向二级 DB）
- [x] M5: backup pipeline 改造（scan 写二级 DB；base-chunk-copy 读 base filemap DB）
- [x] M6: restore/verify 改造（ATTACH 两级 DB；旧格式兼容）
- [x] M7: strict 门禁：去掉 best-effort continue；bootstrap update 失败 => run failed；全量测试回归
- [x] M8: filemap scan 使用 512 条 SQLite 写入事务和单次基线集合查询；普通文件 chunk 与基线 chunk-copy 采用有界多值/集合写入；临时 filemap 使用单写入者 WAL，上传前恢复 FULL sync 并 checkpoint；trace 记录各类 SQLite 累计开销
- [ ] M9: snapshot filemap 演进为 full + delta manifest；首次与压实写 full，v1 保持双读；链深达到 20 或累计压缩 delta 达最近 full 的 25% 时压实，无变化备份 filemap payload 不超过 1 MiB

## Migration State

- Legacy ID-prefixed directory normalized to this slug-only topic.
