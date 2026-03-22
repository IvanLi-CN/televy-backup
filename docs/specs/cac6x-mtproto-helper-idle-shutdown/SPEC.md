# MTProto 空闲 Helper 退出治理（#cac6x）

## 状态

- Status: 已完成
- Created: 2026-03-22
- Last: 2026-03-22

## 背景 / 问题陈述

- 当前 MTProto helper 由 core 进程拉起并在 backup run 期间复用；run 结束后 daemon 会清空 endpoint storage cache，但 helper 进程没有可靠退出协议。
- 在 helper 已失去父进程控制、Telegram 连接进入异常状态时，helper 可能长期残留并在读错误路径上自旋，表现为“当前没有在备份，但 `televybackup-mtproto-helper` 长时间占满一个核心”。
- 该问题出现在正式版稳定运行路径里，会直接破坏用户对“空闲时应零副作用”的基本预期。

## 目标 / 非目标

### Goals

- 让 MTProto helper 在空闲时可靠退出，不再在 daemon idle 窗口中残留后台进程。
- 为 core-helper 内部协议补上显式 `shutdown`，让 drop / respawn 优先走优雅退出，失败时再走 kill fallback。
- 保持现有 primary helper session 持久化、secondary helper session 隔离、upload/download/pin/`wait-chat` 兼容语义不变。
- 为 helper 退出路径补进程级回归测试，防止再次出现 orphan process。

### Non-goals

- 不新增 UI、CLI 或 control IPC 的“停止 helper”入口。
- 不改备份调度、endpoint 配置 schema、Telegram object_id 协议。
- 不把 `wait-chat` 改成新的交互模型；仅治理生命周期。

## 范围（Scope）

### In scope

- helper：新增 `shutdown` 请求；收到 `shutdown` 或 stdin EOF 时，先关闭 sender pool / updates stream，再退出进程。
- core：`MtProtoHelper` 增加统一 teardown 路径；drop 与 respawn 都先尝试 graceful shutdown，超时或异常时 kill fallback。
- daemon：保留现有 `storage_by_endpoint.clear()` 设计，把它作为 idle teardown 的触发点，并为清理动作增加结构化日志。
- tests：新增 helper 进程级退出测试，以及 core drop/fallback 测试。

### Out of scope

- 用户可见的停止按钮、菜单项、CLI 子命令或 control IPC 方法。
- 新的 telemetry 管道或长期后台驻留优化。

## 需求（Requirements）

### MUST

- `TelegramMtProtoStorage` 被 drop 后，不得遗留存活的 `televybackup-mtproto-helper` 子进程。
- helper 收到 `shutdown` 时必须返回协议级确认，并在短超时窗口内退出。
- 若 helper 对 `shutdown` 无响应或退出卡住，core 必须在短超时后执行 kill fallback，不能静默悬挂。
- stdin EOF 仍必须触发 helper 退出，避免父进程异常终止时留下孤儿进程。
- daemon 在清空 MTProto storage cache 时必须记录结构化清理日志，便于追查“为什么 helper 被回收”。

### SHOULD

- teardown 不应引入新的用户可见错误；正常 backup/restore/`wait-chat` 路径无需新增额外配置。
- 回归测试应区分“优雅 shutdown”与“fallback kill”两条路径。

## 功能与行为规格（Functional/Behavior Spec）

### Core flows

- daemon 完成一轮 MTProto backup 后，进入下一次 idle loop 并清空 `storage_by_endpoint`：
  - `TelegramMtProtoStorage` drop；
  - 每个 helper 先发送 `shutdown`；
  - helper 关闭 sender pool / updates stream 并退出；
  - Activity Monitor 不再看到长期存活的 helper。
- core 因 helper 超时/异常需要 respawn 时：
  - 旧 helper 优先走同一套 graceful shutdown；
  - 若旧 helper 已坏死或不响应，则 kill fallback；
  - 新 helper 再按既有 init 协议启动。

### Edge cases / errors

- helper 未初始化也必须接受 `shutdown` 并直接退出，避免 teardown 依赖 init 成功。
- helper 收到 `shutdown` 但不返回确认、或返回确认后仍不退出时，core 在固定短超时后强制 kill。
- 父进程只关闭 stdin 未发送 `shutdown` 时，helper 也必须退出，不得继续保活 sender pool。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `shutdown` helper stdio request | rpc | internal | New | None | core + helper | `TelegramMtProtoStorage` / `MtProtoHelper` | 仅内部 JSON line 协议使用 |
| helper teardown lifecycle | internal | internal | Modify | None | core + helper | daemon idle cleanup / respawn path | 不新增用户可见 surface |

## 验收标准（Acceptance Criteria）

- Given 一轮 MTProto backup 已成功结束
  When daemon 进入下一次 idle loop
  Then 不再残留 `televybackup-mtproto-helper` 进程。
- Given core 主动 drop 或替换 helper
  When helper 正常响应 `shutdown`
  Then helper 返回确认并快速退出，不经 kill fallback。
- Given helper 对 `shutdown` 无响应
  When core 执行 teardown
  Then core 会在短超时后 kill helper，且最终不残留 orphan process。
- Given 父进程仅关闭 stdin
  When helper 主循环读到 EOF
  Then helper 会终止 sender pool 并退出。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- helper: `cargo test --manifest-path crates/mtproto-helper/Cargo.toml -- --nocapture`
- core: `cargo test -p televy_backup_core telegram_mtproto -- --nocapture`

### Quality checks

- Rust: `cargo fmt --all -- --check`

## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: helper 协议新增 `shutdown`，并在 EOF / shutdown 下统一退出 sender pool
- [x] M2: core helper wrapper 改为 graceful shutdown + kill fallback，并覆盖 drop / respawn
- [x] M3: daemon idle cache clear 增加结构化 teardown 日志
- [x] M4: helper/core 补进程级回归测试并通过定向验证

## 方案概述（Approach, high-level）

- 把“helper 进程退出”从隐式依赖 pipe EOF，提升为显式内部协议能力。
- core 端所有 teardown 入口共享同一条 helper 终止路径，避免 respawn、drop、异常恢复行为分叉。
- daemon 不新增新的 stop surface，只继续复用既有 cache clear 生命周期，降低产品面变更。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：
  - 若 shutdown 超时设置过短，极端慢机上可能过早落入 kill fallback；因此需保持“短但不激进”的超时窗口。
  - helper teardown 若处理不当，可能误伤现有 session 持久化时机；需要维持 primary helper 的 session 读写口径不变。
- 假设：
  - daemon loop 中 `storage_by_endpoint` 是正式版空闲 helper 的唯一长期持有者；本轮不扩展到新的用户可见 stop 入口。

## 参考（References）

- [crates/core/src/storage/telegram_mtproto.rs](../../../crates/core/src/storage/telegram_mtproto.rs)
- [crates/mtproto-helper/src/main.rs](../../../crates/mtproto-helper/src/main.rs)

## 变更记录（Change log）

- 2026-03-22：新增 MTProto helper idle teardown 规格，锁定“空闲即退出 helper、无新增用户可见 stop 入口”的实现边界与验收标准。
