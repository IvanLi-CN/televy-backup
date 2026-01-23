# Settings：Endpoints 独立配置页（Targets 仅绑定）（#0009）

## 状态

- Status: 待实现
- Created: 2026-01-23
- Last: 2026-01-23

## 背景 / 问题陈述

- 当前 Settings window 的 Targets 详情里包含一整块 Telegram Endpoint 配置编辑区（含 secrets / session / test connection），不利于“同一个 Endpoint 被多个 Target 复用”的交互与心智模型。
- 期望把 Endpoints 作为独立配置页集中管理；Targets 只负责“绑定 Endpoint”，并展示只读信息。

## 目标 / 非目标

### Goals

- 在 Settings window 增加独立的 `Endpoints` 页面，用于创建 / 删除 / 选择并编辑 Endpoint 配置与连接验证。
- 在 `Targets` 页面：仅允许选择（绑定）Endpoint；下方展示该 Endpoint 的配置摘要（只读、不可编辑）。
- 在 `Targets` 页面提供 `Edit endpoint…` 按钮：跳转到 `Endpoints` 页面，并自动选中对应 Endpoint。

### Non-goals

- 不改动 `config.toml` 的 schema（`telegram_endpoints[]` / `targets[].endpoint_id` 等字段保持不变）。
- 不引入新的 UI 风格体系；延续现有 Settings window 的布局与控件语义。
- 不在本计划中新增“Endpoint 友好名称/Label”字段（若需要，另开计划）。

## 范围（Scope）

### In scope

- Settings window 导航：在现有 segmented control 中加入 `Endpoints` 入口（或等价的同层级导航）。
- `Targets` → Endpoint 绑定区改造：
  - Endpoint Picker 保留；
  - 原 endpointEditor 变为只读摘要（不显示可编辑控件：TextField/SecureField/Save/Clear 等）；
  - 增加 `Edit endpoint…` 跳转按钮，并在跳转后选中对应 endpoint。
- `Endpoints` 页面能力：
  - 左侧列表：展示 endpoints；支持新增与删除；
  - 右侧详情：编辑 endpoint（chat id、bot token、session、test connection）以及必要的全局 Telegram MTProto 信息（api id/hash）；
  - 删除保护：被任一 target 引用的 endpoint 不可直接删除（需先解除引用或走确认流程）。
- 设计图与说明文档：
  - `Targets` 页面（改造后）设计图
  - `Endpoints` 页面设计图
  - 两页的交互说明与跳转规则文档

### Out of scope

- 变更 secrets 存储方式、加密方案或 CLI 子命令行为。
- 端到端自动化 UI 测试体系搭建（如仓库当前无对应框架）。

## 需求（Requirements）

### MUST

- Settings window 存在可访问的 `Endpoints` 页面入口（与 `Targets / Recovery Key / Schedule` 同层级）。
- `Targets` 页面 Endpoint 区域：
  - 允许选择 endpoint（picker）并保存到 target；
  - 下方信息为只读展示（无法直接编辑 chat id / bot token / api hash / sessions 等）；
  - 有 `Edit endpoint…` 入口可进入 `Endpoints` 页面，并自动选中当前 endpoint。
  - 保留 `Test connection` 动作（用于对当前绑定 endpoint 进行验证）。
- Endpoint picker：
  - 下拉列表按“自然文字排序（natural sort）”展示；
  - 新建/进入 `Targets` 详情时，默认选中 endpoint 的规则为：
    - 维护两个“最近时间点”：`最近一次创建/更新的 endpoint` 与 `最近一次被选为 target 的 endpoint`；
    - 取二者中时间更新的那个 endpoint 作为默认值（若不存在则回退到列表首项）。
- `Endpoints` 页面：
  - 支持新增 endpoint（生成新 id，并初始化默认字段）；
  - 支持删除 endpoint（若被 target 引用：先阻止删除并引导用户去 `Targets` 解除/改绑引用；解除后允许删除）；
  - 支持编辑 endpoint 配置，并保留现有 `Test connection`、`Clear sessions`、secrets save 等能力。
- Endpoint `id` 视为不可变：不提供重命名能力。
- 从 `Targets` 跳转到 `Endpoints` 后，应满足：
  - 目标 endpoint 被选中并可见；
  - 返回 `Targets` 后（或切回 Targets tab 后）摘要信息反映最新配置。

## 接口契约（Interfaces & Contracts）

本计划不修改外部接口（CLI / config.toml schema 等保持不变），但会新增一组用于“默认 endpoint 选择”的内部持久化键（UI 偏好设置）。

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| UserDefaults: last endpoint heuristics | Config | internal | New | ./contracts/config.md | macOS app | Settings window | 仅 UI 默认选择逻辑，不进入 config.toml |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/config.md](./contracts/config.md)

## 验收标准（Acceptance Criteria）

- Given Settings window 位于 `Targets`，
  When 选中某个 target 并在 Endpoint picker 中选择某个 endpoint，
  Then 下方展示该 endpoint 的配置摘要且所有编辑控件不可用/不可见。

- Given Settings window 位于 `Targets` 且 target 已绑定 endpoint，
  When 点击 `Test connection`，
  Then 使用该 endpoint 发起验证，并把验证结果反馈在 `Targets` 页（成功/失败/未验证态）。

- Given `Targets` 页面某个 target 绑定了 endpoint，
  When 点击 `Edit endpoint…`，
  Then 跳转到 `Endpoints` 页面并自动选中该 endpoint（列表高亮 + 详情对应）。

- Given `Endpoints` 页面选中某个 endpoint，
  When 修改 `Chat ID` 并保存、保存 bot token、保存 api hash、清除 sessions、执行 `Test connection`，
  Then 行为与现有实现一致（能保存/提示状态），并在切回 `Targets` 时摘要展示更新后的状态。

- Given 某 endpoint 被至少一个 target 引用，
  When 在 `Endpoints` 页面尝试删除该 endpoint，
  Then 删除被阻止，并提供一键跳转到 `Targets` 的入口，且默认选中一个“引用该 endpoint 的 target”以便用户改绑/解除引用（用户手动完成后再回来删除）。

- Given Endpoint picker 有多个 endpoint，
  When 打开下拉列表，
  Then 列表按“自然文字排序”展示（例如 `ep_2` 排在 `ep_10` 之前）。

- Given 存在“最近一次创建/更新的 endpoint”与“最近一次被选为 target 的 endpoint”，
  When 进入 `Targets` 详情并需要一个默认 endpoint，
  Then 默认值取这两个时间点中更近的那个 endpoint（若该 endpoint 不存在则回退到列表首项）。

（补充关键边界与异常：空 endpoint 列表、endpoint 未找到、保存失败、test connection 失败等）

## 实现前置条件（Definition of Ready / Preconditions）

- 交互冻结：默认 endpoint 选择规则、下拉排序与删除被引用 endpoint 的流程已确认
- 验收标准覆盖 core path + 关键边界/异常，且已由主人确认
- 已确认“新增 Endpoint”的默认字段与 id 生成规则（复用现有或调整）

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: 不强制新增（SwiftUI 侧若无现成框架则维持现状）；Rust config 校验不变。
- Integration tests: 手动验收 checklist（见验收标准）覆盖跳转、保存、删除保护、异常路径。
- E2E tests (if applicable): N/A

### Quality checks

- 维持仓库现有的 `cargo test` / lint / 格式化门槛（不引入新工具）。

## 文档更新（Docs to Update）

- `docs/plan/0009:endpoints-settings-page/PLAN.md`: 冻结范围、验收、开放问题
- `docs/plan/0009:endpoints-settings-page/design/README.md`: 设计图说明（交互/跳转规则）

## 方案概述（Approach, high-level）

- 将 endpointEditor 从 `Targets` 详情中解耦：
  - `Targets` 仅保留 picker + 只读摘要；
  - 新增 `Endpoints` 页面承载编辑与动作（save secrets / clear sessions / test connection）。
- 从 `Targets` 跳转到 `Endpoints` 采用“带参数的选择”：
  - `Edit endpoint…` 触发：切换 section=Endpoints，并把 selectedEndpointId 设为当前 target.endpoint_id。
- 删除保护以“引用关系”为准：`targets[].endpoint_id` 指向的 endpoint 禁止删除。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：
  - 当前实现会在删除 target 时自动清理未被引用的 endpoints；在“Targets 默认绑定既有 endpoint + 允许 endpoints 独立存在”的目标下，需要调整清理策略，避免误删或导致 endpoints 无法长期复用。
  - `API ID / API hash` 实际是全局字段，但目前 UI 放在 endpointEditor 中；拆页后需要确认它放在哪一页/哪个分组更清晰。
- 假设（需主人确认）：
  - `Endpoints` 页面是 Settings window 的同层级 tab（segmented control 增加一项）。【已确认】

## 变更记录（Change log）

- 2026-01-23: 创建计划并补齐初版范围/验收与设计产物链接。

## 参考（References）

- 现有实现入口：`macos/TelevyBackupApp/SettingsWindow.swift`
- 现有配置约束：`crates/core/src/config.rs`（`targets[].endpoint_id` 必须引用已存在的 `telegram_endpoints[].id`）
- 旧设计基线：`docs/plan/0005:multi-backup-directories-keyed-restore/PLAN.md`
- 设计图：
  - `design/settings-window-targets.png`（source: `design/settings-window-targets.svg`）
  - `design/settings-window-endpoints.png`（source: `design/settings-window-endpoints.svg`）
