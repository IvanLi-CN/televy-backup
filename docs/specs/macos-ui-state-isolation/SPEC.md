# macOS UI 状态隔离与空闲 CPU 治理

> Canonical topic retained as the canonical source for current product behavior.

> 当前有效规范以本文为准；实现覆盖与当前状态见 `./IMPLEMENTATION.md`，关键演进原因见 `./HISTORY.md`。

## 背景 / 问题陈述

TelevyBackup daemon 在空闲时每秒发出状态心跳。当前 macOS App 将设置、任务、状态、历史和展示状态集中在一个根级 `ObservableObject` 中；即使心跳只改变时间戳，也会使主窗口、Popover、Settings 和 Diagnostics 的 SwiftUI 依赖图整体失效。真实 Release 采样显示高 CPU 位于 SwiftUI AttributeGraph、Observation 和布局更新，而 daemon 本身保持空闲。

本规范定义 UI 状态所有权、status 发布语义和可重复性能验收，避免 wire cadence 与界面刷新 cadence 被错误绑定。

## 目标 / 非目标

### Goals

- 将 status、run history、settings、task/presentation 状态放入独立 Store。
- 将进程控制、CLI 命令和窗口协调保留在非观察 runtime/coordinator。
- idle 心跳只在业务内容或连接阶段变化时发布。
- running 更新合并并限制到最多 2Hz，同时保留最终快照。
- 用真实 bundled daemon 与 status CLI 在隔离 Dev 环境执行 30 秒 CPU 验收。

### Non-goals

- 不修改 daemon/CLI status wire schema、IPC cadence 或 Telegram 行为。
- 不迁移、复制、删除或覆盖生产配置、secret、索引与日志。
- 不恢复旧 Release，不自动合并 PR，不发布 release。
- 不用 UI demo 或空状态流代替真实 daemon fixture。

## 范围（Scope）

### In scope

- macOS SwiftUI 依赖注入和领域 Store。
- status ingest 的等价判断、fresh/stale/disconnected 阶段与 running 节流。
- 两个 disabled target 的无 secret fixture。
- 无 Keychain 的 Dev idle CPU smoke harness 和失败诊断产物。
- 本机 Release 停止、隔离 Dev 启动和进程边界验证。

### Out of scope

- daemon、CLI、Telegram 与备份协议的功能变化。
- 生产数据维护和 release 发布。

## 需求（Requirements）

### MUST

- `AppModel` 或其替代 runtime 不得作为根级 `EnvironmentObject` 注入任何窗口。
- Main Window、Popover、Settings 和 Diagnostics 只能观察自身读取的领域 Store。
- status ingest 必须记录最新原始心跳和接收时间，即使该心跳不触发发布。
- idle 快照仅 `generatedAt`、`receivedAt` 或派生时间变化时不得发布业务状态。
- fresh、stale、disconnected 阶段变化必须恰好发布一次。
- running 快照最多每 500ms 发布一次；窗口内输入必须合并为最新值，结束快照不得丢失。
- 性能实例必须使用 bundle id `com.ivan.televybackup.dev`、`TELEVYBACKUP_DISABLE_KEYCHAIN=1` 和当前 worktree 的 `.dev/perf-idle/{config,data}`。
- harness 必须运行两个 disabled/idle target、bundled daemon 和 status CLI，并采样 GUI 30 秒。
- GUI 平均 CPU 必须低于 5%，峰值必须低于 20%。
- harness 失败必须非零退出、保留 Dev 实例，并生成 CPU 序列和 `sample` 报告。
- 停止 Release 前必须按 PID 确认 bundle id、可执行路径和 config/data 参数；只能停止确认过的 `com.ivan.televybackup` 进程。

### SHOULD

- 同一逻辑提交只产生一次对应 Store 的 `objectWillChange`。
- fixture 与 harness 应可在干净 worktree 重复运行，且不含 secret 或外部网络依赖。

### COULD

- 性能报告可附带 daemon 与 status CLI 的 CPU 序列，辅助定位非 GUI 回归。

## 功能与行为规格（Functional/Behavior Spec）

### Core flows

1. runtime 接收 status 行并解码为快照。
2. Status Store 无条件更新内部最新心跳时间，并计算连接阶段。
3. idle 语义等价时不发布；业务字段或连接阶段改变时发布一个新状态。
4. running 首个快照可立即发布，其后 500ms 窗口内只保留最新快照；计时器到期发布最新值。
5. running 结束或转 idle 时立即提交最终快照并清空待发布值。
6. 窗口通过非观察 runtime 执行命令，通过所需 Store 接收状态。

### Edge cases / errors

- 心跳持续到达时，连接阶段以最新接收心跳为依据，不因被抑制的 `generatedAt` 显示为 stale。
- status stream 中断后，阶段依次跨越 stale 与 disconnected；每个边界只发布一次。
- harness 任一步进程身份、目录隔离、stream 或 CPU 校验失败都不得宣告通过。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Domain stores | Swift API | internal | New | 本文 | macOS App | SwiftUI windows | Status、RunHistory、Settings、Task/Presentation |
| App runtime | Swift API | internal | Modify | 本文 | macOS App | SwiftUI commands/AppDelegate | 非观察命令协调器 |
| idle CPU harness | shell CLI | internal | New | 本文 | macOS tooling | developers/CI-capable hosts | 隔离 Dev 实例 |

### 契约文档（按 Kind 拆分）

- 本主题不新增外部接口；内部合同由本文完整定义。

## 验收标准（Acceptance Criteria）

- Given 两个业务等价的 idle 快照，When 仅心跳时间改变，Then Status Store 发布计数不增加。
- Given 持续 running 输入，When 在 500ms 窗口内到达多个快照，Then 最多发布一次且最终值可见。
- Given 四类窗口被构建，When 检查依赖注入，Then 不存在根级 `AppModel` 环境订阅且命令仍可执行。
- Given 隔离 fixture 和真实 bundled daemon，When 采样 Dev GUI 30 秒，Then 平均 CPU <5%、峰值 <20%，且无生产目录写句柄。
- Given CPU 或隔离校验失败，When harness 退出，Then 返回非零、诊断产物存在且 Dev 实例继续运行。

## 验收清单（Acceptance checklist）

- [x] 核心路径的长期行为已被明确描述。
- [x] 关键边界/错误场景已被覆盖。
- [x] 涉及的接口/契约已写清楚或明确为内部接口。
- [x] 相关验收条件已经可以用于实现与 review 对齐。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: idle 等价抑制、连接阶段边界、running 2Hz 合并与最终快照。
- Integration tests: 四类窗口 Store 注入、runtime 命令接口和无根级 AppModel 环境订阅。
- E2E tests: 真实 Dev GUI + bundled daemon + status CLI 的 30 秒 CPU smoke。

### UI / Storybook (if applicable)

- 本主题不改变视觉设计；不新增 Storybook 或视觉基线。

### Quality checks

- `scripts/macos/swift-unit-tests.sh`
- `TELEVYBACKUP_APP_VARIANT=dev scripts/macos/build-app.sh`
- `scripts/macos/idle-cpu-smoke.sh`

## Visual Evidence

PR: none

## Related PRs

- None

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：SwiftUI 对同一 Store 任意 `@Published` 变化都会使订阅视图失效，因此高频诊断事件不得与业务状态共享发布面。
- 风险：macOS `ps` CPU 采样存在短时抖动，harness 在启动预热后连续采样 30 秒并同时约束平均与峰值。
- 需要决策的问题：None。
- 假设：当前开发基线为 `origin/main@be4a31c9e054bd4db62f562b9e4d3c91a81ba225`。

## 参考（References）

- `./IMPLEMENTATION.md`
- `./HISTORY.md`
