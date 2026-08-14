# 备份请求队列与前置阶段可观测性演进历史

> 这里记录会影响 Agent 理解“为什么一步步变成现在这样”的关键演进；单次任务流水账不放这里，规范正文仍以 `./SPEC.md` 为准。

## Decision Trace

- 以 `backup.enqueue` control RPC 取代文件触发：文件轮询无法确认接收、无法表达合并关系，且让 UI 的本地反馈与 daemon 真实调度脱节。
- 固定“活动批次 + 至多一个后续批次”：满足重复点击的可预期反馈，同时避免无边界队列与持久化需求。
- 固定严格目标串行：不在本主题引入并发连接、预扫描或上传竞争。
- 在 Telegram connect 前发布 `connecting`：把原本约十秒的无状态前置等待变为真实可观察的运行阶段。
- Main Window 和 Popover 对 Connecting 统一为 inline spinner；Prepare 继续沿用既有 indeterminate backup bar，避免把连接等待伪装成传输进度。
- 移除主操作中的“再排一批”图标：它没有可靠的通用含义。运行或排队时主操作统一为 Stop backup，并通过 `backup.stop` 取消当前 daemon 任务和清空手动队列，而不是停止 daemon。

## Key Reasons / Replacements

- 取代 `control/backup-now` 的写文件、daemon 轮询、消费和 stale-drop 逻辑。
- 引用而不替代 `z324m-unified-backup-progress-prepare` 的 Prepare/进度口径。

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
