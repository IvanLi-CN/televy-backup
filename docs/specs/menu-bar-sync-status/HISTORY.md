# macOS 菜单栏同步状态与传输速率演进历史

> 这里记录影响长期理解的决定原因；规格正文仍以 `./SPEC.md` 为准。

## Decision Trace

- 状态由显式任务种类和方向驱动，而不是从 phase、瞬时速率或历史运行记录猜测。
- 失败采用 10 秒、客户端内存的 live latch，避免重启或历史失败在菜单栏持续告警。
- 传输速率是本机显示偏好，不属于可移植备份配置。
- 菜单栏速率使用等宽四字符槽，而不是详细速率字符串；由此消除速率数值、精度和量级变化造成的状态项宽度抖动，同时保留实时读数。
- 同一目标一律互斥，跨目标允许并发并用全局方向聚合。
- 外部任务终态不是 fire-and-forget：只有匹配应用或精确幂等重放可以确认，避免 CLI 在 daemon 未记录终态时报告成功。
- 通用恢复和验证必须解析唯一目标后准入；无法归属的 snapshot 不以绕过互斥的方式继续运行。
- 右键快捷菜单不以任意 `running` 推断 Stop Backup，而是复用明确 backup activity 或 manual queue；退出 GUI 与完全退出继续保持 daemon 所有权边界。

## Key Reasons / Replacements

- 取代以 `state=failed` 或 `lastRun` 直接决定菜单栏错误的做法。
- 为未来原生双向同步预留声明式 `sync` 活动，而不新增菜单栏状态分支。

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
