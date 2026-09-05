# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认“全局上下行速率/累计流量”的数据口径（业务 bytesUploaded/bytesDownloaded）并冻结。
- 已确认 targets 行不展示 per-target 下行字段（仅上行）。
- 已确认状态数据源：daemon 落盘 `status.json`（路径/原子写/刷新频率）→ CLI `status stream`（输出 NDJSON），以及刷新频率上限。
- 已确认 Dev 视图入口对所有用户可见。
- Repo reconnaissance（已完成，供实现落点定位）：
  - `macos/TelevyBackupApp/TelevyBackupApp.swift`：`PopoverRootView` 当前为 header + `OverviewView()`（无 tabs/Logs）；本计划主要替换 Overview 为“全局 network + 多 target 列表”。
  - `macos/TelevyBackupApp/TelevyBackupApp.swift`：当前 `popover.contentSize = NSSize(width: 360, height: 460)`；本计划要求“宽 360、高度自适应（max 720）”，实现需按 targets 内容动态调整 content size（溢出时列表滚动）。
  - `macos/TelevyBackupApp/TelevyBackupApp.swift`：Settings window 已通过 header gear 打开；本计划要求在 Settings 内“增加 Developer… 入口”（不新增 Settings 页面），点击打开独立 Developer window。


## 非功能性验收 / 质量门槛（Quality Gates）

### Performance

- 运行中 UI 更新目标：`2Hz`（500ms；允许 ±10% 更新时间误差）；静止态可降至 `1Hz`（以不显著增加功耗为准）。
- 不允许通过高频启动短命 CLI 进程实现实时（避免 CPU/电量与抖动）；实时路径应为“单一长连接/长进程 stream”或等价机制。
- daemon 侧状态快照（`status.json`）写入需要限频（建议上限 10Hz）并保证单条快照体积可控（避免 IO/CPU 抖动）。
- Scroll UX：targets 列表在可滚动时，顶部/底部应提供渐隐提示；实现应使用“内容 alpha mask”（而非覆盖一层带颜色的遮罩），以兼容 popover 半透明材质。
- 渐隐显示规则：仅当列表可滚动时启用；未到顶部时显示顶部渐隐、未到底部时显示底部渐隐（到顶/到底关闭对应边）。
- Layout UX：Popover 高度需随内容自适应；当 targets 数量较少时，Popover 不应强制拉到最大高度；当 targets 溢出时，高度达到上限并启用列表滚动（不得出现内容贴边或溢出圆角）。
- List Insets：targets 列表需要明确 `contentInsetTop/contentInsetBottom`（推荐 bottom≥16px），确保首/尾行在滚动边缘不会贴边或被圆角裁切。

### Testing

- Contract tests（Rust）：对 `StatusSnapshot` schema（序列化/字段缺失容错）与 `status stream` NDJSON 格式做单测。
- UI smoke（macOS）：验证 Overview/Dev 的刷新频率、stale 提示、单位/舍入规则一致性。


## 实现里程碑（Milestones）

- [x] M1: 定义并实现状态数据源（`status get/stream` + `StatusSnapshot`）
- [x] M2: Popover Overview 重做（全局网络 + 多 target 列表 + 进度/状态）
- [x] M3: Popover Dev 视图落地（全局 + per-target 原始字段展示）
- [x] M4: 测试与文档更新（契约测试 + UI smoke + IA 文档）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0010:status-popover-dashboard/PLAN.md`.
