# 状态面板：补齐 Verify/Restore 下行速率（down）（#mycnc）

## 状态

- Status: 待实现
- Created: 2026-02-06
- Last: 2026-02-06

## 背景 / 问题陈述

- macOS UI 的 Popover Overview 中 `NETWORK`（Up/Down）与 Targets 行尾速率，来源于 `televybackup --json status stream` 的 `status.snapshot`。
- 现象：在 GUI 触发 `verify`/`restore` 时，操作系统可观测到明显网络流量，但 UI 中 `Down`/Targets 速率长期显示 `—`。
- 已确认根因：
  - `status stream` 侧仅基于 `bytesUploaded` 计算 `up.bytesPerSecond`/`upTotal`，`down` 始终输出 `null`；
  - `verify`/`restore` 的实际下载量未被作为独立指标贯通到 `status.snapshot`，且 CLI 执行的任务进度未回写到 daemon 的 status 视图。

## 目标 / 非目标

### Goals

- 后端提供**真实可用**的下载口径：
  - core 统计并输出 `bytesDownloaded`（至少覆盖：index manifest/parts + chunk/pack 下载）。
  - daemon/status stream 能把下载进度映射为 `status.snapshot.global.down`（速率 + session total）。
- GUI 触发的 `backup/verify/restore`（CLI 执行）在任务运行期间，Popover 的 `NETWORK` 区块能显示 `Down` 实时速率与 Session 累计值（不再长期 `—`）。

### Non-goals

- 不把 `verify/restore` 改为 daemon 直接执行（仍由 GUI spawn CLI）。
- 不改动 UI 的 stale 判定口径（仍以 `generatedAt` 为准）。

## 范围（Scope）

### In scope

- `televy_backup_core`：
  - `TaskProgress` 新增 `bytes_downloaded`（additive）。
  - restore/verify 路径补齐下载累计与 progress 透传。
- daemon：
  - `status.json` / status IPC 的 per-target `progress.bytesDownloaded` 可被更新（兼容缺省）。
  - control IPC 增加 best-effort 方法，允许 CLI 将正在运行的任务进度回写到 daemon 的 status 视图（仅用于 UI status 展示，不改 secrets/权限边界）。
- CLI：
  - `--events` 任务在运行时，best-effort 上报 task start/progress/finish 给 daemon（socket 不存在或方法缺失时静默降级）。
  - `status stream` enrich 逻辑补齐 `global.down`（速率 + session totals），并保持向后兼容（additive）。

### Out of scope

- OS 级别的全局网卡流量统计（仍以业务下载/上传字节口径为准）。
- 引入新的 long-lived 服务或额外守护进程。

## 验收标准（Acceptance Criteria）

- Given 用户在 macOS UI 中对某 target 点击 `Verify` 或 `Restore`,
  When 任务执行产生下载流量，
  Then Popover `NETWORK` 中 `Down` 在任务运行期间显示非 `—` 的速率值，并且 `Session` 累计随任务推进增长。

- Given daemon 未运行或 control IPC 不可用，
  When 用户执行 `verify/restore`，
  Then CLI/GUI 仍可正常执行任务（仅 status 面板的 down 指标可能缺失），且不会引入明显卡顿（best-effort 上报短超时）。

## 测试与验证（Test plan）

- 单元测试：
  - core：restore/verify 的 `TaskProgress.bytes_downloaded` 单调递增（含 pack cache 不重复计数）。
  - status stream：down totals/rates 在输入快照 bytesDownloaded 增长时被正确填充。
- 最小手动验证：
  - 运行 daemon + 打开 Popover；
  - 触发一次 verify（下载）并观察 `Down` 显示与累计增长。

## 风险与注意事项

- 进度上报为 best-effort：需避免在 progress sink 中引入长阻塞（短超时 + 节流）。
- `bytesDownloaded` 为“业务下载字节”口径，包含加密/封装开销；但应与 UI 展示目的（带宽观察）一致且稳定。

