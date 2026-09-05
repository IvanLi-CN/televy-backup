# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `README.md`: 说明 prepare 阶段与进度条语义。
- `docs/architecture.md`: 补充并行 prepare 与进度字段口径。


## 方案概述（Approach, high-level）

- 在 backup 前置阶段引入并行任务编排，并把 prepare 作为统一阶段对外暴露。
- 将 UI 进度逻辑组件化，剥离散落在多个视图中的重复判断与样式。
- 保持接口变更 additive，避免要求状态源与客户端同版本升级。


## 变更记录（Change log）

- 2026-02-25: 新建规格并冻结并行 Prepare + 双段进度口径。
- 2026-02-25: 完成实现并通过 `cargo test`、`cargo test -p televybackup`、`cargo test -p televybackupd`、`scripts/macos/build-app.sh` 验证。
- 2026-02-27: 增补 `Need Upload (Disc./Final)`、`Remaining`、`Saved` 口径定义，并要求 UI 按阶段切换文案避免歧义。
- 2026-02-28: 将 runtime 进度视觉规范明确为单条多层（`NeedUploadConfirmed <= UploadingCurrent <= BackedUp <= Scanned`），并与当前实现/文案一致。
