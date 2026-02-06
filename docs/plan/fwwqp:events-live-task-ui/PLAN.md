# CLI events 实时状态与 GUI 进度一致性修复（flush + progress）（#fwwqp）

## 状态

- Status: 已完成
- Created: 2026-02-05
- Last: 2026-02-05

## 背景 / 问题陈述

- 目前 macOS GUI 通过 `televybackup --events ...` 消费 stdout NDJSON events（`task.state` / `task.progress`）来展示任务状态与进度。
- 现象：任务开始/失败/完成的状态更新不及时，甚至需要等到任务结束后才出现状态与记录，用户体验较差。
- 已确认根因之一：在 `--events` 模式下，部分 `task.state` 输出未强制 `flush`，当 stdout 被 pipe 接管时可能发生块缓冲，导致 UI 迟到。
- 已确认根因之二：GUI 对 `task.progress` 的字段消费不完整（例如 verify/restore 主要更新 `bytesRead/chunksDone`），导致“看起来没动”。

## 目标 / 非目标

### Goals

- `--events` 模式下，CLI 对每一条事件输出提供可靠交付语义：**每行 NDJSON 都可被 UI 及时读到**（避免因缓冲导致“结束才到”）。
- GUI 在 backup/verify/restore 全流程中展示“可见、可信”的进度（至少每秒有一次可见更新）。
- GUI 对于 CLI 触发的 verify/restore：不再仅依赖 run history 推断 running，任务开始后可在短时间内直接显示 running。

### Non-goals

- 不把 verify/restore 改为 daemon 执行（仍由 GUI spawn CLI）。
- 不重做 daemon status snapshot（IPC/status.json）协议，仅修复 CLI events 与 UI 消费链路。
- 不引入新的远端观测/上报系统（保持本地 NDJSON 日志体系不变）。

## 范围（Scope）

### In scope

- CLI（Rust，`crates/cli/`）：
  - 统一事件输出函数：写 stdout + 换行 + `flush`（不污染 stderr，不混入 run log）。
  - 在 `--events` 模式下补齐 `task.state` 的 failed 事件（含 error 信息，遵循 `events.md` 兼容规则：只新增字段）。
- macOS app（Swift，`macos/TelevyBackupApp/`）：
  - 完整消费 `task.progress` 字段（`bytesRead/chunksDone/filesDone/...`），并将其映射到 UI 展示。
  - 引入轻量 “active task” 状态机：按 `taskId` 幂等更新 UI（running/progress/succeeded/failed），并在主界面立即体现。

### Out of scope

- 变更 core 的任务进度产生逻辑（保持现有 `TaskProgress` 结构；必要时仅在 CLI 增强输出语义）。
- UI 展示完整逐行日志明细（依旧以摘要 + 打开日志文件为主）。

## 需求（Requirements）

### MUST

- 兼容性：
  - 不破坏 `televybackup --events` stdout NDJSON 合约（只输出 events）。
  - 允许新增字段；不删除/重命名现有字段（见 `docs/plan/0001:telegram-backup-mvp/contracts/events.md`）。
- 体验阈值：
  - 点击 Verify/Restore/（CLI 触发的）Backup 后 **<= 300ms**：UI 显示 running（目标行/详情页至少一处）。
  - 任务执行中 **<= 1s**：UI 至少一次可见更新（phase/bytes/chunks 任一维度）。
  - 任务结束后 **<= 1s**：UI 显示 succeeded/failed，toast 正确，run history 刷新并归档到正确 target（或 Unknown target 兜底）。
- 失败可见：
  - 失败时 UI 能尽量在进程退出前收到 `task.state=failed`（减少“只有 exit 才知道失败”）。

### SHOULD

- `task.state` 的 failed/succeeded 事件带上最小可展示信息（例如 error code/message、snapshotId、targetId），便于 UI 做更准确的提示。
- 对 `task.progress` 输出做轻量节流（例如 5-10Hz），避免极端高频进度导致不必要的 CPU 开销（不影响可靠性）。

## 验收标准（Acceptance Criteria）

- Given 用户在主界面点击某个 target 的 `Verify`，
  When CLI 以 `--events` 输出事件，
  Then UI 在 300ms 内显示该 target “Running”，并在执行中持续更新进度；完成后 1s 内出现完成 toast 与 history 记录。

- Given 用户在主界面点击某个 target 的 `Restore...` 并选择空目录，
  When restore 执行中输出进度，
  Then UI 显示 “written/checked” 等与 restore/verify 相匹配的进度指标，而非仅显示 uploaded/deduped。

- Given 配置错误导致任务快速失败（例如 `config.invalid`），
  When CLI 返回非 0，
  Then UI 显示 failed（包含 error code 或明确的失败提示），且不会出现“等待很久才显示失败”的体验。

## 非功能性验收 / 质量门槛（Quality Gates）

- 至少补齐对应的单元测试（CLI events 输出行为、UI 解析逻辑的最小覆盖）。
- 不引入长驻进程/服务启动变更。

## 里程碑（Milestones）

- [x] M1: CLI `--events` 事件输出统一封装并强制 flush（含 failed 事件）
- [x] M2: macOS GUI 完整消费 `task.progress` 字段并展示 restore/verify 进度
- [x] M3: macOS GUI 引入 active task 状态机，running/finish 及时可见，端到端验收通过
