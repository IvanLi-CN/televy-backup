# macOS 菜单栏同步状态与传输速率

> 当前有效规范以本文为准；实现覆盖与当前状态见 `./IMPLEMENTATION.md`，关键演进原因见 `./HISTORY.md`。

## 背景 / 问题陈述

现有状态快照只表达目标的粗粒度 `state`、进度和瞬时速率。菜单栏不能可靠区分备份、恢复与验证，零速准备阶段和跨目标并发也会造成误判；历史失败记录还不能作为当前菜单栏错误。

本规范定义 daemon 到 macOS App 的显式活动契约，以及由该契约投影出的菜单栏状态。它让任务意图而非阶段名、速率或历史记录成为唯一状态依据。

## 目标 / 非目标

### Goals

- 在 `StatusSnapshot.targets[]` 增量公开可选 `activeTask`，声明任务种类和传输方向。
- 菜单栏稳定显示空闲、错误、备份中、恢复中、验证中和双向同步。
- 将当前 live 会话失败锁存 10 秒，且不从历史结果、禁用目标或重启后的快照恢复。
- 保持同一目标上的备份、恢复和验证互斥，允许跨目标并发并汇总为双向同步。
- 提供默认关闭、仅存于本机 `UserDefaults` 的菜单栏全局速率显示偏好。

### Non-goals

- 不改变备份、恢复、验证的数据传输算法、调度策略或速率计算公式。
- 不把菜单栏偏好写入 `config.toml`、导入导出包或远端存储。
- 不支持第一版中同一目标的多任务并发，也不依赖动画或颜色表达状态。

## 范围（Scope）

### In scope

- core status/control 协议、daemon runtime 活动态及外部任务终态。
- daemon 目标互斥准入和手动备份队列的延后执行。
- macOS 菜单栏纯投影、失败锁存、静态模板徽标、速率标题和设置偏好。
- Rust/Swift 自动化测试与受控菜单栏渲染证据。

### Out of scope

- 每个目标的下载速率菜单栏展示。
- 新的原生同步传输实现；本规范仅预留 `sync` 契约。
- 历史运行列表或目标页面既有失败语义的重做。

## 需求（Requirements）

### MUST

- `activeTask` 必须是 `StatusSnapshot` 的可选 additive 字段；旧快照在 Rust 与 Swift 解码后仍可用。
- `activeTask.kind` 只能是 `backup`、`restore`、`verify` 或 `sync`，`directions` 只能包含去重的 `up` 与 `down`。backup、restore、verify 的声明方向分别为 `["up"]`、`["down"]`、`[]`；sync 声明双向。
- 菜单栏状态优先级固定为：当前 live 失败锁存、双向同步、备份、恢复、验证、空闲。备份队列成员等同于备份活动。
- 失败锁存只可由 App 已在本次连接中观察到的活动任务转为失败触发，并在 10 秒后失效。`lastRun`、初次收到的 `state=failed`、App 重启和 daemon 重启都不得触发它。
- 同一目标已有活动任务时，外部 `status.taskStart` 必须以稳定的 `target_busy` 错误拒绝；它不得启动恢复或验证的数据面工作。已在同目标运行外部任务时，排队备份必须保持等待，直至可获得该目标。
- 跨目标并发活动必须在全局投影中聚合；上行和下行同时存在时必须显示双向同步。
- 速率偏好默认关闭，键名为 `showMenuBarTransferRates`，只读 `global.up` 与 `global.down`。启用后只显示活动任务声明方向中速率有效的方向；错误徽标不得隐藏其他活动任务的速率标题。

### SHOULD

- 速率为零仍保留由 `activeTask` 或备份队列决定的活动状态。
- Dev 徽标和状态徽标应占用不重叠的固定位置，并保持模板图像语义。
- 所有菜单栏状态映射应位于可脱离 AppKit 测试的纯 Swift 类型中。

### COULD

- 原生同步实现可通过 `kind=sync` 和双向 `directions` 无需修改菜单栏投影而接入。

## 功能与行为规格（Functional/Behavior Spec）

### Core flows

1. daemon 启动备份或接受外部任务时，为对应目标发布 `activeTask`；完成时清除该字段并记录外部任务的成功或失败终态。
2. macOS App 从 fresh status snapshot 聚合 `activeTask`、备份队列和本地 CLI 事件。它只将已见活动态后的失败转变交给内存失败锁存。
3. 菜单栏先选择唯一状态，再选择匹配状态的静态模板徽标。速率标题独立于徽标，来自 `global` 的有效方向。
4. 设置开关写入 `UserDefaults`；App 重启后读取同一偏好，而 daemon、配置包和远端配置不感知该值。

### Edge cases / errors

- 初始快照中存在历史 `state=failed` 或 `lastRun.status=failed` 时，菜单栏仍按当前活动或空闲显示。
- 同时备份与恢复不同目标、或一个 `sync` 声明双向时，显示双向同步；单独验证显示验证中。
- 失败锁存期间若仍有活动任务，错误徽标优先，但已启用的有效方向速率继续显示。
- unknown/old `activeTask` 不得使 App 崩溃；其字段按可选值容错，旧客户端忽略新增字段。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `targets[].activeTask` | status snapshot | internal | Modify | `./contracts/status-activity.md` | daemon/core | CLI, macOS App | additive-only |
| `status.taskStart` / `status.taskFinish` | control RPC | internal | Modify | `./contracts/status-activity.md` | daemon/core | CLI | task admission and terminal error code |
| `showMenuBarTransferRates` | macOS preference | local | New | `./contracts/status-activity.md` | macOS App | macOS App | UserDefaults only |

### 契约文档（按 Kind 拆分）

- [`./contracts/status-activity.md`](./contracts/status-activity.md)

## 验收标准（Acceptance Criteria）

- Given 缺少 `activeTask` 的旧快照，When Rust 或 Swift 解码，Then 保持可用且没有活动菜单栏状态。
- Given 同时存在上行与下行活动，When 投影菜单栏，Then 显示双向同步；零速不得改变活动状态。
- Given 当前连接已观察到活动任务，When 它转为失败，Then 错误显示 10 秒；初始历史失败不得触发错误。
- Given 同一目标有 daemon 备份，When 外部恢复或验证申请开始，Then 收到 `target_busy` 且数据面不启动；不同目标可以同时工作。
- Given 速率偏好关闭或开启，When 全局速率有效，Then 分别隐藏或仅显示声明方向的 `↑`/`↓` 二进制单位速率。

## 验收清单（Acceptance checklist）

- [x] 核心路径的长期行为已被明确描述。
- [x] 关键边界/错误场景已被覆盖。
- [x] 涉及的接口/契约已写清楚。
- [x] 相关验收条件已经可以用于实现与 review 对齐。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Rust：status JSON 兼容性、控制 IPC 入参/终态和目标互斥。
- Swift：状态矩阵、失败锁存、偏好持久化和速率标题。
- Full：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all-features`、`scripts/macos/swift-unit-tests.sh`、`scripts/macos/build-app.sh`。

### UI / Storybook (if applicable)

- 通过受控 AppKit 菜单栏预览渲染空闲、上传、下载、双向、验证和错误状态；不需要 Storybook。

## Visual Evidence

PR: include

受控菜单栏预览状态。

![Menu bar activity states](./assets/menu-bar-activity-states.png)

## Related PRs

- None

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：`activeTask` 和备份队列的运行权威必须保持在 daemon，App 只能投影。
- 风险：失败锁存属于客户端内存状态，任何持久化都会把历史失败误报为当前错误。
- 需要决策的问题：None。
- 假设：App、CLI 与 daemon 作为同一发行物同步升级，但旧 status snapshot 仍可能在开发与诊断中出现。

## 参考（References）

- `../../plan/0010:status-popover-dashboard/contracts/events.md`
- `../../plan/0011:daemon-status-ipc/contracts/events.md`
- `../../../CONTEXT.md`
- `../../adr/0001-explicit-menu-bar-activity-directions.md`
