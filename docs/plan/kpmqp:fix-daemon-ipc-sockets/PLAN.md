# 修复 daemon IPC 可靠性（解锁 Recovery Key/Verify）（#kpmqp）

## 状态

- Status: 已完成
- Created: 2026-01-30
- Last: 2026-01-30

## 背景 / 问题陈述

- macOS GUI 的 Settings → Recovery Key 页面出现 `Missing/—`，但同一台机器的备份仍然成功。
- GUI 侧调用 CLI 时，出现 `daemon.unavailable`（vault IPC 连接失败）与 `control.unavailable`（control IPC 不可用），导致无法查询 secrets presence、无法导出 TBK1、无法执行 verify。
- 当前现象会误导用户将“IPC 不可用”理解为“密钥丢失”，并降低可恢复性与可排障性。

## 目标 / 非目标

### Goals

- 恢复并稳定 daemon 的 IPC（vault/control），使 GUI/CLI 在 daemon 运行时可可靠读取 secrets presence 与执行 verify。
- GUI 对“IPC 不可用”和“master key 确实缺失”的展示区分清晰，避免误报 `Missing`。
- 为该问题补齐可重复验证的验收口径与测试门槛，避免回归。

### Non-goals

- 不改变 TBK1（recovery key）格式与语义。
- 不重做 secrets/vault 的安全边界与存储方案（仍遵循现有 daemon-only 边界与 Keychain 策略）。
- 不引入新的外部后端/服务。

## 范围（Scope）

### In scope

- daemon：确保启动后会创建并监听 control IPC 与 vault IPC（含对异常/冲突/残留 socket 的处理与可观测性）。
- CLI：`settings get --with-secrets` 在 IPC 不可用时维持“settings 可读 + secretsError 可发现”的行为；在 IPC 可用时返回 secrets。
- macOS GUI：Recovery Key/Verify 的错误态展示与操作 gating（基于 `secretsError` / `daemon.unavailable` 的可操作提示）。

### Out of scope

- 迁移历史数据或重置用户 secrets（除非明确由用户触发）。
- 变更 IPC 协议（字段/方法/路径）或 error code 集合（本计划目标是修复可用性与一致性）。

## 需求（Requirements）

### MUST

- 当 `televybackupd` 运行且使用同一 `TELEVYBACKUP_DATA_DIR` 时：
  - `ipc/control.sock` 存在且可连接（control IPC 可用）。
  - `ipc/vault.sock` 存在且可连接（vault IPC 可用）。
- `televybackup --json settings get --with-secrets`：
  - IPC 可用时：`secrets` 必须为 object（非 null），且不包含 `secretsError`。
  - IPC 不可用时：`secrets` 为 null 且包含 `secretsError{code,message,retryable}`（不得把“不可用”静默当成“缺失”）。
- macOS GUI 的 Recovery Key 页面：
  - 仅当 `masterKeyPresent=false`（从 secrets presence 得到）时，才显示 `Missing`（红色）。
  - 若 `secrets` 不可用（存在 `secretsError` 或 IPC 错误）：显示 `Unavailable`（或等价文案）并提供可操作提示（例如“daemon 未运行/IPC 不可用”），不得误报 `Missing`。
- Verify 操作：
  - 在 daemon 运行时不应因 `daemon.unavailable`（vault IPC）立即失败。
  - 若确实不可用：错误提示必须包含“下一步动作”（例如启动/重启 daemon）与必要的定位信息（至少包含 socket path 或对应 data dir）。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Control IPC socket + `secrets.presence` | RPC | internal | Modify | ./contracts/rpc.md | daemon | CLI, macOS app | 仅修复可用性与一致性，不改协议 |
| Vault IPC socket + `vault.get_or_create` | RPC | internal | Modify | ./contracts/rpc.md | daemon | CLI, macOS app | 仅修复可用性与一致性，不改协议 |
| `televybackup settings get --with-secrets` | CLI | internal | Modify | ./contracts/cli.md | CLI | macOS app | IPC 不可用时的输出约定 |

### 契约文档（按 Kind 拆分）

- [contracts/rpc.md](./contracts/rpc.md)
- [contracts/cli.md](./contracts/cli.md)

## 验收标准（Acceptance Criteria）

- Given `TELEVYBACKUP_DATA_DIR` 指向同一目录且 `televybackupd` 处于运行中
  When 运行 `televybackup --json settings get --with-secrets`
  Then 输出包含 `secrets.masterKeyPresent` 等字段，且不包含 `secretsError`。
- Given `televybackupd` 未运行（或 IPC 不可用）
  When 运行 `televybackup --json settings get --with-secrets`
  Then 输出 `secrets: null` 且包含 `secretsError{code,message,retryable}`。
- Given IPC 不可用（`secretsError` 存在）
  When 打开 macOS GUI → Settings → Recovery Key
  Then 页面显示为“不可用/Unavailable”而非“Missing”，且 Reveal/Copy/Export 被禁用并给出可操作提示。
- Given IPC 可用且 `masterKeyPresent=true`
  When 在 Recovery Key 页面点击 Reveal/Copy/Export
  Then 可展示/复制/导出以 `TBK1:` 开头的 recovery key（不要求默认明文展示）。
- Given `televybackupd` 运行且 IPC 可用
  When 在 GUI 点击 Verify（latest）
  Then verify 流程可开始执行；不得立即以 `daemon.unavailable` 失败。

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认本计划的 UI 文案口径：`Missing` 仅代表 `masterKeyPresent=false`；IPC 不可用使用 `Unavailable`（或等价文案）。
- 已确认 daemon 的 socket “自愈策略”允许在启动时清理残留 socket 并重试绑定（不改变协议/权限边界）。
- 已确认 GUI 允许在 `Reveal/Export/Verify` 前执行 daemon preflight（失败再提示）。
- 已确认测试统一用 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 规避 Keychain 交互（见下文 Quality Gates）。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Integration tests: 增加一条“daemon 启动后 IPC socket 可用”的测试（至少覆盖 `control.sock` 与 `vault.sock` 的可连接性）。
- E2E tests (if applicable): 补一条覆盖 GUI 侧“IPC 不可用 → 显示 Unavailable”的最小验收脚本/步骤（不要求自动化 UI 测试，但必须可复现）。
  - 约束：测试运行时统一设置 `TELEVYBACKUP_DISABLE_KEYCHAIN=1`，避免 Keychain 交互导致不稳定与提示弹窗。

### Quality checks

- 不引入新工具；沿用仓库现有 `cargo test`/现有脚本（具体命令在 impl 阶段确定并写入对应文档/脚本）。

## 文档更新（Docs to Update）

- `README.md`: 增加一段 Troubleshooting：当 Recovery Key 显示不可用/verify 报 `daemon.unavailable` 时，优先检查 daemon 是否运行以及 `TELEVYBACKUP_*_DIR` 是否一致。
- `docs/architecture.md`（如已有相关小节）：补充“IPC endpoints（control/vault/status）”的职责边界与定位入口（仅文档层面说明）。

## 资产晋升（Asset promotion）

None

## 实现里程碑（Milestones）

- [x] M1: 复现用例与回归测试（IPC sockets 可连接）
- [x] M2: daemon：修复/增强 control+vault IPC 的启动可靠性与可观测性
- [x] M3: macOS GUI：Recovery Key/Verify 错误态区分与可操作提示
- [x] M4: 文档补齐（Troubleshooting + IPC 说明）

## 方案概述（Approach, high-level）

- 优先修复“daemon 启动后未监听 control/vault IPC”的根因（绑定失败、残留 socket、权限/路径不一致、生命周期被提前 drop 等）。
- GUI 侧不再把“secrets 不可用（control IPC 不可用）”等价成“密钥缺失”；以 `secretsError` 作为第一手信号。
- 对 verify：在执行前做一次 daemon 可用性 preflight（或让 UI 触发 daemon 启动/重启），失败时给出可执行的下一步动作。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：IPC 的失败原因可能与外部环境有关（多实例/launchd/权限/残留文件）；需要在实现中补齐更明确的诊断信息。
- 假设：不改变 IPC 协议与 error code 形状，仅修复“可用性 + UI 口径”。

## 变更记录（Change log）

- 2026-01-30: 冻结关键决策（UI 文案口径 + daemon 自愈 + GUI preflight + 测试禁用 Keychain），状态切换为 `待实现`
- 2026-01-30: 完成实现（daemon IPC 启动鲁棒性 + GUI preflight/Unavailable + Troubleshooting 文档）；等待主人验收后推进后续合并/PR

## 参考（References）

- 相关入口（代码定位）：
  - `crates/daemon/src/main.rs`（IPC servers spawn）
  - `crates/daemon/src/control_ipc.rs`
  - `crates/daemon/src/vault_ipc.rs`
  - `crates/cli/src/main.rs`（`settings get --with-secrets`）
  - `macos/TelevyBackupApp/SettingsWindow.swift`（Recovery Key UI）
  - `macos/TelevyBackupApp/TelevyBackupApp.swift`（daemon lifecycle & command runner）
