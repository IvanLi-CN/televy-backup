# macOS UI 状态隔离与空闲 CPU 治理演进历史

> 这里记录会影响 Agent 理解“为什么一步步变成现在这样”的关键演进；单次任务流水账不放这里，规范正文仍以 `./SPEC.md` 为准。

## Decision Trace

- 2026-08-13：真实 Release 采样将高 CPU 定位到 SwiftUI 全窗口失效，而非 daemon 工作负载。
- 2026-08-13：选择领域 Store 真拆分；idle 采用语义发布，running 采用 2Hz 有界合并。
- 2026-08-13：性能验收固定为无 Keychain、工作区隔离的真实 Dev GUI/daemon/status stream，失败保留现场。

## Key Reasons / Replacements

- 用 UI 发布策略替代改变 daemon 心跳 cadence，保留 status wire 与 CLI 行为。
- 用非观察 runtime 加按需 Store 注入替代根级 `AppModel` 环境订阅，缩小 SwiftUI 失效范围。

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
