# History

## Provenance

- Legacy source: `docs/plan/0001:telegram-backup-mvp/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/requirements.md`: 与本计划冻结后的范围/术语/验收标准对齐（避免重复与冲突）。
- `README.md`: 补充“运行方式、配置入口、数据目录与风险提示（Telegram 存储风险）”。
- `docs/architecture.md`: 必须新增并记录核心数据流与威胁模型。


## 方案概述（Approach, high-level）

- 分层：`core`（chunk/encrypt/index）与 `app`（native macOS UI + `televybackup` CLI）解耦；存储通过 `Storage Adapter` 抽象以降低切换成本。
- 最小传输：chunk 级去重优先；如果 Telegram 支持通过 `file_id` 复用对象，则实现“零上传复用”路径。
- 一致性口径：以“扫描时刻的一致性”为目标（不做 APFS snapshot）。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0001:telegram-backup-mvp/PLAN.md`.
