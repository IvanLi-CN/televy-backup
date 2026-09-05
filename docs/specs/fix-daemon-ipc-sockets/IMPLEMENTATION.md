# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

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


## 实现里程碑（Milestones）

- [x] M1: 复现用例与回归测试（IPC sockets 可连接）
- [x] M2: daemon：修复/增强 control+vault IPC 的启动可靠性与可观测性
- [x] M3: macOS GUI：Recovery Key/Verify 错误态区分与可操作提示
- [x] M4: 文档补齐（Troubleshooting + IPC 说明）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/kpmqp:fix-daemon-ipc-sockets/PLAN.md`.
