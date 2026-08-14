# macOS UI 状态隔离与空闲 CPU 治理实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖、交付进度与 rollout 相关事实，避免这些细节散落到 PR / Git 历史里。

## Current Status

- Implementation: 已完成
- Lifecycle: active
- Catalog note: 领域 Store、发布策略、fixture 与 CPU harness 已实现并通过隔离 Dev 验收。

## Coverage / rollout summary

- 已确认高 CPU 栈位于 SwiftUI AttributeGraph/Observation/layout，daemon 本身为空闲。
- 已确认当前 Release 心跳约 1Hz，空闲快照主要变化为时间字段。
- `AppModel` 已成为非观察 runtime；Status、RunHistory、Settings、Task/Presentation 与 Diagnostics 状态分别由 Store 持有。
- idle 语义等价快照不发布；running 输入以 500ms 窗口合并并保留最终快照。
- 真实 bundled daemon/status CLI fixture 含两个 disabled target，全程使用 `.dev/perf-idle` 和 `--disable-keychain`。
- 30 秒主窗口 idle CPU 实测平均 0.04%、峰值 0.30%；Dev 实例在验收后保留。
- 验收前未发现运行中的 Release 进程，因此未发送停止信号；生产目录元数据哈希前后保持一致。

## Remaining Gaps

- 创建 PR 并收敛至 live Step 5C Ready。

## Related Changes

- None

## References

- `./SPEC.md`
- `./HISTORY.md`
