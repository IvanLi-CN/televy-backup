# Popover Targets 高度实时自适应与误滚动修复（#2e73n）

## 状态

- Status: 已完成
- Created: 2026-02-25
- Last: 2026-03-05

## 背景 / 问题陈述

- 当前 Popover 高度由 `targetCount * 固定行高` 估算，未使用真实列表内容高度。
- Running 行摘要在长文案时会换行，导致单行高度超过估算值，触发 1/2 target 场景的误滚动。
- 误滚动会让用户误判为“达到上限”，与面板实际可扩展空间不一致。

## 目标 / 非目标

### Goals

- 以真实列表内容高度驱动 Popover/Targets 高度。
- 修复 1 target / 2 targets（含 Running）场景误滚动。
- 保持 `minHeight=320`、`maxHeight=720`；仅在超上限时滚动。
- 运行态摘要固定单行，减少高度抖动。

### Non-goals

- 不修改 CLI/daemon 状态接口与 schema。
- 不修改 Main window / Settings window 布局。
- 不引入新配置项或运行时开关。

## 范围（Scope）

### In scope

- `macos/TelevyBackupApp/TelevyBackupApp.swift` 的 Popover Overview/Targets 布局高度算法。
- Targets 列表内容高度测量回传与 Popover 高度更新逻辑。
- Running 行摘要单行截断策略。

### Out of scope

- Rust crates 下的 status 数据流、IPC、文件格式。
- 非 Popover 视图的 UI 排版。

## 需求（Requirements）

### MUST

- Targets 容器高度优先使用实时测量值；测量不可用时可回退估算。
- Popover 高度必须随列表真实高度变化实时更新。
- 高度更新须包含抖动抑制（小于阈值不更新）。
- Running 摘要文案必须单行且尾部截断。

### SHOULD

- 保持现有 stale/disconnected/empty state 语义不变。
- 保持滚动渐隐逻辑与视觉风格不回退。

### COULD

- 对列表 inset 做轻量优化，减少首尾贴边感。

## 功能与行为规格（Functional/Behavior Spec）

### Core flows

- Popover 打开后，Overview 根据 Targets 实际内容高度设置列表容器高度。
- 列表内容变化（如 Running 文案变化）时，实时重算并同步 Popover 高度。
- 当目标数量与内容高度超过上限时，Popover 固定 720，由列表承担滚动。

### Edge cases / errors

- 状态尚未到达（`snap == nil`）时保留 waiting empty state 高度策略。
- 无 targets 时使用 empty state 高度，不显示误滚动。
- 测量值暂不可用时使用估算值，并在测量可用后收敛到真实值。

## 接口契约（Interfaces & Contracts）

None（无外部接口变更）。

## 验收标准（Acceptance Criteria）

- Given 1 个 Running target，When 打开 Popover，Then 在未达上限时无持续可见滚动条。
- Given 2 个 targets（Running + Idle），When 状态文案刷新，Then 高度实时重算且不误触发滚动。
- Given 多 targets 超过可视区，When 打开 Popover，Then Popover 封顶 720 且列表可滚动。
- Given stale/disconnected/empty state，When 打开 Popover，Then 状态语义与文案保持一致。

## 实现前置条件（Definition of Ready / Preconditions）

- 范围、非目标、验收标准已冻结。
- UI 变更仅限 Popover 路径，且不改外部契约。
- 允许走快车道推进到 PR + checks + review-loop 收敛。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- `scripts/macos/build-app.sh` 成功。
- `scripts/macos/swift-unit-tests.sh` 成功。
- 手工验证 A/B/C/D 场景（1 target、2 targets、多 targets、empty/stale/disconnected）。

### Quality checks

- 变更文件通过仓库现有构建检查；不引入 `--no-verify` 提交。

## 文档更新（Docs to Update）

- `docs/specs/README.md`：新增规格索引。
- `docs/specs/2e73n-popover-targets-live-height/SPEC.md`：记录口径与验收。

## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: 接入 Targets 真实内容高度测量并回传布局层。
- [x] M2: Popover 高度改为实测驱动并增加抖动抑制。
- [x] M3: Running 摘要单行截断并完成场景回归验证。
- [x] M4: 创建 PR 并完成 checks + review-loop 收敛。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：SwiftUI 布局反馈链可能造成短时重排；通过阈值抑制降低震荡。
- 开放问题：无。
- 假设：overlay scrollbar 的瞬时显示属于系统行为，不计入误滚动。

## 变更记录（Change log）

- 2026-02-25: 创建规格并冻结修复口径。
- 2026-02-25: 完成实现并推送 `th/2e73n-popover-targets-live-height`，PR #48 更新到 `0d63324`。
- 2026-02-25: CI run #251 (`.github/workflows/ci.yml`) 成功；review-loop 结论为无阻塞问题。
- 2026-03-05: Popover 高度改为 `NSHostingController.sizeThatFits(in:)` 实测驱动，补齐 SwiftUI 布局尺寸测量自动化测试并接入 macOS CI，防止高度回归与多余空白复现。
