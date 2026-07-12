# Daemon 生命周期与可控退出 演进历史（#k7d2v）

> 这里记录影响长期理解的关键演进；规范正文仍以 `./SPEC.md` 为准。

## Decision Trace

- 保留 `televybackupd` 作为共享 scheduled backup daemon。
- App 退出与 daemon 完全退出分离；schedule 已启用时必须由用户选择。
- 完全退出的优雅等待上限固定为十秒，LaunchAgent 必须 unload 以阻止 keep-alive 重启。
- 完全退出期间，状态弹窗保留原位运行状态：全部操作控件禁用，退出入口显示活动指示器；停止失败时取消 App 退出并提供恢复路径。

## Key Reasons / Replacements

- 本主题补齐此前仅覆盖 MTProto helper 的生命周期治理，不替代 helper idle shutdown 规范。

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
