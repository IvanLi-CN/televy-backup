# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认 Config Bundle 必须“自包含可导入”（只导入一个文件即可导入）。
- 已确认 overwrite remote 的语义：仅更新 pinned 指针（不删除远端对象）。
- 已确认 MTProto session 不导出：导入后按需重新生成并落盘。
- 已确认 “local-vs-remote mismatch” 的判定口径（以每个 target 对应 endpoint 的 `index.<endpoint_id>.sqlite` 记录为依据是否足够）。
- 已确认导入 apply 的“索引重建”策略：备份旧 db → 拉取远端最新可用索引落盘为新 db（或 bootstrap missing 时建空库）。
- 已确认 index 按 endpoint 隔离 + 禁止 chat 复用（见计划 `#r6ceq`），从而避免“multi-endpoint 场景下索引来源选择”的歧义。
- 已确认迁移期兼容策略：旧全局 `index.sqlite` 存在时静默忽略；导入 apply 仅处理 per-endpoint `index.<endpoint_id>.sqlite`（见 `#r6ceq`）。
- 已确认二次确认的具体交互形态：typed phrase（输入 `IMPORT`），并明确“哪些动作”需要额外的二次确认（例如 overwrite remote / overwrite master key）。
- 已确认 master key mismatch 的策略：
  - 无 targets：允许 apply（但需二次确认）
  - 有 targets：进入 `#4fexy` 的轮换流程（可暂停/继续/取消，成功后切换）


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit（Rust / core）：
  - bundle encode/decode round-trip
  - schema/version mismatch 行为
  - secrets 覆盖范围与缺失标注
- Integration（CLI）：
  - export → import(dry-run) → import(apply) 的 happy path
  - 冲突场景：missing_path / bootstrap_missing / bootstrap_invalid / local-vs-remote mismatch
- Manual（macOS GUI）：
  - 导入 UI：摘要展示、targets 默认全选、多选与冲突决策交互

### Quality checks

- `cargo fmt` / `cargo clippy` / `cargo test`（按仓库既有 CI 约束）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/fn4ny:config-bundle-export-import/PLAN.md`.
