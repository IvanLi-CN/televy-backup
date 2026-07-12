# Daemon 生命周期与可控退出 实现状态（#k7d2v）

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖、交付进度与 rollout 相关事实。

## Current Status

- Implementation: 部分完成
- Lifecycle: active
- Catalog note: daemon、CLI、macOS App 与 LaunchAgent 的统一退出治理。

## Coverage / rollout summary

- daemon control IPC 新增 `daemon.stop`，活动 scheduled backup 绑定取消 token。
- CLI 新增 `daemon start|status|stop`。
- macOS App 新增退出图标与统一终止回调；完整退出请求 daemon 停止并尝试卸载 LaunchAgent。
- 完整退出期间显示阻塞式收尾状态；daemon 未在十秒内停止时取消 App 退出、恢复 App 状态流与计时器，并保留失败说明。
- 完整退出先卸载 LaunchAgent 以阻止 keep-alive 重启；remote index preflight 也接入任务取消 token。

## Remaining Gaps

- CLI daemon-dependent 命令的自动临时拉起尚未实现。
- 需要补充活动任务取消与 LaunchAgent 完全退出的进程级集成测试。

## Related Changes

- None

## References

- `./SPEC.md`
- `./HISTORY.md`
