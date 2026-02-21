# macOS：移除 Developer window，把 Diagnostics 整合进主界面（#7f9wg）

## 状态

- Status: 已完成
- Created: 2026-02-19
- Last: 2026-02-19
- Notes: PR #45

## 背景 / 问题陈述

当前 GUI 同时存在：

- Main window（面向用户）：按 target 聚合 restore/verify 与 run history；
- Developer window（偏排障）：展示 status snapshot 原始字段 + activity + Copy JSON/Reveal/Freeze。

但 Developer window 与主界面信息结构重复，且入口在 Settings 内，割裂了“看历史/看诊断”的工作流。希望将其整合到主界面，移除独立 Developer window，降低 UI 复杂度并提升可发现性。

## 目标 / 非目标

### Goals

- 移除独立 Developer window（不再保留窗口与入口）。
- 在 Main window 的 target detail 内新增 `History | Diagnostics` 分段：
  - `History` 为默认分段，保持现有行为；
  - `Diagnostics` 展示：
    - `GLOBAL`（schemaVersion/generatedAt/source/rates/totals/receivedAt/freshness）
    - `TARGET DETAILS`（targetId/endpointId/enabled/state/phase/progress 原始计数/bytes 原始值/lastRun 摘要）
    - `ACTIVITY`（含 filter 输入框 + 最近 200 条）
- 保留 Developer window 工具动作中的 `Copy JSON` + `Reveal…`，并在 Diagnostics 内提供。
- 移除 `Freeze`（不再提供暂停刷新）。
- 可见性：Diagnostics 对所有构建 Always 可见（不做隐藏开关）。

### Non-goals

- 不改动 status snapshot schema（仅调整 GUI 展示与入口组织）。
- 不新增第三个窗口（不引入新的独立 Diagnostics window）。
- 不重画/重命名已有 SVG 设计资产文件（仅同步文档描述，避免继续引用“Developer window”作为产品入口）。

## 范围（In/Out）

### In scope

- SwiftUI/AppKit GUI：
  - 移除 Settings 工具栏的 `Developer…` 入口。
  - 移除 `DeveloperWindowRootView` 与相关窗口管理代码。
  - Main window（TargetDetailView）增加 segmented control 与 Diagnostics 内容。
  - 继续支持 `Copy JSON`（复制当前 `StatusSnapshot`）与 `Reveal…`（定位 `status.json`）。
  - Activity：在不刷屏的前提下，补齐包含 targetId 的关键事件（state/lastRun 变化），便于默认按 targetId 过滤。
- 文档同步：
  - `docs/design/system-architecture.md`
  - `docs/design/ui/statusbar-popover-ia.md`
  - `docs/design/ui/statusbar-popover-dashboard/README.md`

### Out of scope

- CLI/daemon 行为或接口变更。
- CI 工作流/发布流程变更。

## 验收标准（Acceptance Criteria）

### UI

- Given 用户打开 Settings window
  - Then 工具栏不再出现 `Developer…` 按钮。
- Given 用户打开 Main window 并选中某个 target
  - Then 详情页可切换 `History | Diagnostics` 分段，且默认选中 `History`。
- Given 用户切到 `Diagnostics`
  - Then `GLOBAL/TARGET DETAILS/ACTIVITY` 可见且显示稳定（缺失字段统一显示 `—`，不崩溃）。
- Given 用户在 `Diagnostics` 点击 `Copy JSON`
  - Then 剪贴板得到 pretty-printed JSON，并显示 toast `Copied status JSON`。
- Given 用户在 `Diagnostics` 点击 `Reveal…`
  - Then Finder 打开并选中 `status.json`（即便文件暂不存在也不崩溃）。
- Given 用户在 UI 内尝试寻找 Freeze
  - Then UI 不再提供 Freeze 开关，快照持续实时刷新。

### 工程/质量

- `scripts/macos/build-app.sh`（dev variant）构建成功。
- `cargo fmt -- --check` / `cargo clippy` / `cargo test` 在本地可运行通过（对齐 CI 质量门槛）。

## 测试计划（Test Plan）

- 必跑：`TELEVYBACKUP_APP_VARIANT=dev scripts/macos/build-app.sh`
- 建议：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all-features`

## 风险与注意事项

- `TelevyBackupApp.swift` 为大文件：删除 Developer window 相关代码需谨慎避免破坏括号/结构。
- `StatusFreshness` 当前为 `fileprivate`，Diagnostics 若跨文件复用 freshness 逻辑，需要调整可见性或复制常量；应避免常量漂移。
- Activity 默认按 targetId 过滤：需要确保至少部分 activity 文案包含 targetId，否则可能出现“空列表”错觉。

## 里程碑（Milestones）

- [x] 移除 Developer window 与 Settings 入口
- [x] Main window 增加 `History | Diagnostics` 分段 + Diagnostics 内容
- [x] 文档同步（architecture + IA + UI README）
- [x] 本地验证通过 + PR 创建 + CI checks 结果明确
