# 备份请求队列与前置阶段可观测性实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖、交付进度与 rollout 相关事实，避免这些细节散落到 PR / Git 历史里。

## Current Status

- Implementation: complete
- Lifecycle: implemented
- Catalog note: control IPC、daemon batch coordinator、macOS 状态投影与 UI demo 已在同一主题分支实现。

## Coverage / rollout summary

- `backup.enqueue` 是 App 的全量、单目标和导入后合并本地目录入口；CLI 仅负责将 scope 传给 control IPC，`backup run` 保持独立直跑语义。
- daemon 实现活动批次与唯一后续批次，按 settings 顺序去重并投影 `backupQueue.activeBatchId` / `pendingBatchId`。
- 既有 `z324m` Prepare 和确定性进度语义保持不变；本主题只增加连接前与队列成员投影。
- Popover 和 Main Window 共用 `TargetPresentation`：`Starting` 为本地桥接，`Queued` 来自 daemon membership，Connecting 使用 inline spinner。
- 开发模式首次启动会在 control IPC 可达前完成既有的自动主密钥初始化；App 仅在 control 与 vault IPC 均可连接后提交 enqueue，并用当前快照立即确认已返回的 batch，避免冷启动和状态去重造成的假失败。
- 队列只存活于 daemon 进程内，daemon 重启后不恢复未开始请求。

## Validation evidence

- Rust: `cargo fmt --all -- --check`, `cargo check --workspace --all-targets`, daemon queue tests, control RPC contract tests, and CLI enqueue IPC test.
- macOS: `scripts/macos/swift-unit-tests.sh` covers StatusStore, TargetPresentation, request-button state, existing popover layout, demo sandbox, and diagnostics settings.
- UI demo: `scripts/macos/capture-backup-queue-ui.sh <light|dark> target/ui-evidence/backup-queue` captures only the demo Popover and Main Window. The main-window helper binds capture to the launched demo PID and refuses full-screen fallback.

## Visual evidence

PR: include

- [`light-connecting-queued-popover.png`](./assets/light-connecting-queued-popover.png): light Popover, connecting target plus queued follower.
- [`light-connecting-queued-main-window.png`](./assets/light-connecting-queued-main-window.png): light Main Window, inline Connecting feedback and queued sidebar target.
- [`dark-running-next-queued-popover.png`](./assets/dark-running-next-queued-popover.png): dark Popover, running target with `Next queued` and a queued follower.
- [`dark-running-next-queued-main-window.png`](./assets/dark-running-next-queued-main-window.png): dark Main Window, running header with `Next queued`.

## References

- `./SPEC.md`
- `./HISTORY.md`
