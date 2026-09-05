# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 目标/非目标、范围（in/out）、约束已明确
- 远端 bootstrap/catalog 的“可发现方式”已由主人确认（例如 pinned message vs 其他机制）
- config schema v2 与 provider namespace 规则已冻结（或明确迁移策略）
- 金钥导入/导出 UX（仅 CLI vs UI 也提供入口）已由主人确认
- CLI 恢复入口（按 source/target vs 按 snapshot_id）已冻结


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests:
  - config schema v2 parse/validate（含 v1 兼容读取）
  - provider namespace 生成规则稳定性
  - 金钥导入/导出（长度/格式校验；不覆盖已有 key 的策略）
  - bootstrap/catalog JSON schema + 加解密 round-trip（使用 InMemoryStorage）
- Integration tests:
  - 使用 InMemoryStorage 跑一轮 backup → 生成 bootstrap/catalog → 在“空 data_dir”场景下完成 restore/verify

### Quality checks

- 按仓库既有约定执行 fmt/lint/test（不引入新工具）。


## 实现里程碑（Milestones）

- [x] M1: `config.toml` schema v2（targets + endpoints + per-target schedule）与 v1 兼容读取/迁移写回
- [x] M2: provider namespace 变更：`telegram.mtproto/<endpoint_id>`（多 endpoint 去重隔离）
- [x] M3: multi-endpoint secrets + validate：每个 endpoint 的 token 写入 secrets store、按 endpoint validate
- [x] M4: bootstrap/catalog：加密 catalog 文档上传 + pin root pointer + resolve latest（供 restore/verify 使用）
- [x] M5: restore/verify 新入口：`latest`（按 `target_id` 或 `source_path`）在新设备无旧 SQLite 下可恢复
- [x] M6: Settings window UI（targets/endpoints/schedule/recovery key）+ Popover 导航最小改动（移除 Settings tab + gear 打开 Settings window）
- [x] M7: daemon 按 target schedule 触发（默认继承全局；override 生效）+ 多 endpoint 支持
- [x] M8: tests + docs updates（覆盖 config/crypto/bootstrap；更新 README/architecture）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0005:multi-backup-directories-keyed-restore/PLAN.md`.
