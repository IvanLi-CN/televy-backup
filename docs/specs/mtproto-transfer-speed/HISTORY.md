# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## 方案概述（Approach, high-level）

- 使用 Telegram 推荐的 512KiB part size，减少协议开销并提升单次请求载荷。
- 统一 FloodWait 解析与退避逻辑，把 Premium wait 视为同级别的“必须等待后重试”信号。
- 引入全局冷却：用 `next_allowed` 的 max 合并策略，让并发 worker 在任何一个触发 FloodWait 时都统一降速，避免“并发继续撞限速”的指数放大。
- 在等待阶段输出进度心跳，保证 core 的 helper watchdog 不误杀。
- 在 core 侧引入 helper pool，让并发 upload job 真实落到多个 helper 进程；并通过“仅 primary helper 更新持久化 session”的策略规避 session 并发冲突。


## 变更记录 / Change log

- 2026-03-02：补齐 core 的 helper pool 以支持并行 upload jobs；并增加多 helper 进程间 session 隔离（仅 primary helper 复用/更新持久化 session），修复并行下可能的卡死/超时。
