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
- `televybackup gui quit` uses a dedicated GUI listener, stopped lease, and lifecycle lock to hand off one data directory without touching the daemon. GUI-only exits preserve daemon-owned work; complete exits track and cancel GUI-owned local command processes before exact-environment daemon shutdown.
- 完整退出期间显示阻塞式收尾状态；daemon 未在十秒内停止时取消 App 退出、恢复 App 状态流与计时器，并保留失败说明。
- 完整退出仅在 release 默认环境 disable LaunchAgent 以阻止 keep-alive 重启，并将 CLI stop 指向 Formula 的 config/data 目录，优雅停止后再 bootout；App 等待 CLI 十秒窗口加返回余量；dev/custom-dir 不变更 release 服务；bootout 失败会取消 App 退出；remote index preflight 与 post-backup bootstrap update 均接入任务取消 token，CLI start 脱离调用终端。

## Remaining Gaps

- CLI daemon-dependent 命令的自动临时拉起尚未实现。
- 需要补充活动任务取消与 LaunchAgent 完全退出的进程级集成测试。
- 需要补充 GUI-only handoff、stopped-lease idempotency、以及 complete-exit fail-closed 的进程级集成测试。

## Related Changes

- None

## References

- `./SPEC.md`
- `./HISTORY.md`
