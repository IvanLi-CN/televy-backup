# macOS 菜单栏同步状态与传输速率演进历史

> 这里记录影响长期理解的决定原因；规格正文仍以 `./SPEC.md` 为准。

## Decision Trace

- 状态由显式任务种类和方向驱动，而不是从 phase、瞬时速率或历史运行记录猜测。
- 失败采用 10 秒、客户端内存的 live latch，避免重启或历史失败在菜单栏持续告警。
- 传输速率是本机显示偏好，不属于可移植备份配置。
- 同一目标一律互斥，跨目标允许并发并用全局方向聚合。

## Key Reasons / Replacements

- 取代以 `state=failed` 或 `lastRun` 直接决定菜单栏错误的做法。
- 为未来原生双向同步预留声明式 `sync` 活动，而不新增菜单栏状态分支。

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
