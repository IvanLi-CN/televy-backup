# PR + Label 发布能力

> Canonical topic retained as the canonical source for current product behavior.
> This historical topic is superseded by [`product-version-release-chain`](../product-version-release-chain/SPEC.md). Its frozen-intent and backfill design is retained only for historical context; the VERSION-only contract is authoritative.

## 背景 / 问题陈述

- 现状：仓库只有单一 `CI` workflow，`main` push 会直接按本地版本号自动补丁递增并发 tag/release。
- 问题：发布意图没有显式来源，PR 合并后无法稳定区分 `patch/minor/major`、`stable/rc` 与 `docs/skip`。
- 风险：连续 merge 到 `main` 时，若 CI/release 共享一条可抢占流水线，容易出现发布决策不透明或漏发版。


## 目标 / 非目标

### Goals

- 用 PR labels 作为唯一发布意图来源：`type:*` + `channel:*`。
- PR 阶段提前做 label gate，未知/冲突标签直接失败。
- 拆分 `CI (PR)`、`CI (main)`、`Release` 三条 workflow：
  - PR CI 可抢占；
  - main CI 不抢占；
  - Release 跟随 `CI (main)` 成功结果或手动 backfill。
- Release 通过 merge commit 反查唯一 PR，并按标签决定：
  - 是否发版；
  - semver bump 级别；
  - stable / rc tag 形式。
- 保持 tag 创建与 Release 创建幂等，可安全重跑。

### Non-goals

- 不拥有 macOS 制品打包或多架构构建；这些由 [`macos-release-distribution`](../macos-release-distribution/SPEC.md) 统一定义。
- 不变更现有 Rust/macOS 测试矩阵内容。
- 不自动 merge PR。


## 行为规格（Behavior Spec）

### PR 标签契约

- 目标为 `main` 的 PR 必须且只能有一个 intent label：
  - `type:patch`
  - `type:minor`
  - `type:major`
  - `type:docs`
  - `type:skip`
- 目标为 `main` 的 PR 必须且只能有一个 channel label：
  - `channel:stable`
  - `channel:rc`
- 任意未知 `type:*` / `channel:*`，或同类标签缺失/冲突，PR label gate 直接失败。

### CI / Release 编排

- `CI (PR)`：`pull_request` 触发，`concurrency.cancel-in-progress=true`。
- `CI (main)`：`push(main)` + `workflow_dispatch` 触发，`concurrency.cancel-in-progress=false`。
- `Release`：
  - 默认由 `workflow_run` 监听 `CI (main)` 成功完成；
  - 允许 `workflow_dispatch(head_sha)` 做补发，但 `head_sha` 必须可证明属于 `main` 历史。
  - `Release` workflow 自身必须串行化，避免不同 merge commit 并发计算出同一个 stable semver。
- `CI (main)` 成功后必须上传冻结的 release intent artifact（基于当时的 PR labels 解析结果）。
- `Release` 与 manual backfill 只能消费冻结的 release intent artifact；artifact 缺失时保守跳过。

### 版本与 tag 规则

- 版本基线取当前最大稳定 semver tag（`vX.Y.Z` 或 `X.Y.Z`），若不存在则回退 `crates/daemon/Cargo.toml` 版本。
- `type:patch/minor/major` 分别做对应 bump。
- stable tag 形式：`vX.Y.Z`。
- rc tag 形式：`vX.Y.Z-rc.<sha7>`。
- `type:docs` / `type:skip`：不发 tag、不建 release。

### 幂等与保守策略

- tag 已存在时跳过创建，不视为失败；若并发窗口中被其他 run 先创建，也应视为成功。
- GitHub Release 使用可重跑的更新模式。
- GitHub API 反查 PR 或读取 labels 失败时，Release 输出 skip reason，而不是盲目发版。


## 验收标准（Acceptance Criteria）

- PR 缺少或冲突 `type:*` / `channel:*` 时，label gate 失败。
- merge 到 `main` 后，`Release` 能基于 merged PR labels 正确区分 stable / rc / skip。
- `compute-version.sh` 不再只做“Cargo.toml patch 自增”，而是按最大稳定 tag + bump level 计算下一版本。
- 所有新增脚本具备最小本地回归测试，并纳入 CI。
- 仓库需声明 PR merge gate 的 `required_checks`，并记录任何临时 waiver。
