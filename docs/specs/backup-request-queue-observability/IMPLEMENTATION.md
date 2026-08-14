# 备份请求队列与前置阶段可观测性实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖、交付进度与 rollout 相关事实，避免这些细节散落到 PR / Git 历史里。

## Current Status

- Implementation: 进行中
- Lifecycle: active
- Catalog note: control IPC、daemon batch coordinator、macOS 状态投影与 UI demo 正在同一主题分支实现。

## Coverage / rollout summary

- 新实现会删除 `control/backup-now` 的生产、轮询、消费与 stale-drop 路径。
- 既有 `z324m` Prepare 和确定性进度语义保持不变；本主题只增加连接前与队列成员投影。
- 队列只存活于 daemon 进程内，daemon 重启后不恢复未开始请求。

## Remaining Gaps

- 完成 Rust control/daemon/CLI 测试与实现。
- 完成 Swift 共享展示模型、按钮状态和 UI demo 场景。
- 完成亮/暗色限定窗口证据、完整验证及 PR 收敛。

## Related Changes

- None

## References

- `./SPEC.md`
- `./HISTORY.md`
