# PR + Label 发布能力（#n2kbu）

## 状态

- Status: 已完成
- Created: 2026-03-07
- Last: 2026-03-07

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

- 不引入额外制品打包或多平台构建发布。
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

## 质量门槛（Quality Gates）

- `bash ./.github/scripts/test-release-scripts.sh`
- `cargo test --all-features`
- `cd crates/mtproto-helper && cargo test`
- `bash scripts/macos/swift-unit-tests.sh`

## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: 新增 `PR Label Gate` workflow 与标签校验脚本
- [x] M2: 拆分 `CI (PR)` / `CI (main)`，并保留现有测试矩阵
- [x] M3: 新增 `release-intent.sh`，支持 merge commit -> PR -> labels 决策
- [x] M4: 改造 `compute-version.sh` 为 semver bump 驱动
- [x] M5: 新增 `Release` workflow，支持 stable/rc/skip 与手动 backfill
- [x] M6: 补充脚本合同测试与 README 发布规则说明

## Change log

- 2026-03-07：按 `style-playbook` 的 `pr-label-release` 参考落地 label gate、拆分 CI 与 label-driven release。
- 2026-03-07：补充 release backfill 的 `main` ancestry 校验，并让 tag 并发竞争按幂等成功处理。
- 2026-03-07：将 `Release` workflow 改为全局串行，避免不同 merge commit 并发抢占同一 stable 版号。
- 2026-03-07：在 `CI (main)` 冻结 release intent artifact，避免 rerun/backfill 被 merge 后改标签污染。
