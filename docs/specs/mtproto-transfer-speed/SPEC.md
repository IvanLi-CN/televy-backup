# MTProto 备份传输提速：更大分片 + 更正确的 FloodWait 处理 + 可调并发/节流

> Canonical topic retained as the canonical source for current product behavior.

## 背景 / 问题陈述

- 现象：MTProto 备份阶段“带宽吃不满”，整体传输慢，尤其在默认配置 `min_delay_ms=250` 时更明显。
- 根因（当前已确认）：
  - helper 端上传/下载默认按 **128KiB** 分片；在存在 `min_delay_ms` 节流时，请求频率被限制，导致吞吐上限偏低。
  - core 已有 helper pool，但 MTProto Storage 的 async future 在首次 poll 内同步等待 helper stdin/stdout；同一个 `FuturesUnordered` 无法继续 poll 其他 worker，故配置为 2 时实际 document upload 仍会交替执行。
- 风险：在提升并发/降低延迟时，Telegram 服务端可能返回 `FLOOD_WAIT_X` / `FLOOD_PREMIUM_WAIT_X`，若处理不当会导致更严重的限速或抖动。


## 目标 / 非目标

### Goals

- 在不违反 Telegram 文件传输约束的前提下，显著提升 upload/download 吞吐。
- 对 `FLOOD_WAIT_X` 与 `FLOOD_PREMIUM_WAIT_X` 做正确解析与退避重试，避免并发继续打满导致更严重限速。
- 提供可调的并发与节流参数（上传/下载），并在 macOS UI 中暴露“高级”调节入口与风险提示。
- 将 `crates/mtproto-helper` 的测试纳入 CI，避免“主仓全绿但 helper 挂了”。

### Non-goals

- 引入多连接（multi-sender）。
- 在多个 helper 进程之间并发共享同一个 MTProto session/auth key（不安全）。
- 重写 restore 的并行下载策略（本次只做 chunk size 提升与 FloodWait 兼容）。


## 范围（Scope）

### In scope

- helper：上传/下载 part size 默认提升到 **512KiB**（Telegram 官方推荐且在约束内）。
- helper：补齐 `FLOOD_PREMIUM_WAIT_X` 解析，并与 `FLOOD_WAIT_X` 统一进入可重试/退避逻辑。
- helper：新增“全局冷却（global cooldown）”机制：任一 worker 收到 FloodWait，会抬高全局 `next_allowed`，使所有并发分片一起停下来等待。
- helper：在长时间等待（cooldown）期间保持周期性 progress 事件心跳，避免 core 侧误判 helper 卡死（core 的 upload event 超时为 45s）。
- core：把 `FLOOD_PREMIUM_WAIT` 视为 transient/可重试错误；并纳入 flood-wait 检测（用于降档/退避）。
- core：实现 MTProto helper pool：由 `max_concurrent_uploads` 控制 helper 进程池大小，使并发上传 job 不再被单 helper stdin/stdout 串行化。
- core：多 helper 并行时做 session 隔离：仅 primary helper 复用/更新持久化 session，其余 helper 使用独立 session（通过 bot token 重新鉴权），避免并发共享 session 导致卡死或异常。
- core：direct、pack、index-part 使用同一个有界上传调度器，使
  `max_concurrent_uploads` 成为所有 core 可观测 document RPC 的全局上限；index manifest 必须等待
  parts 完成后上传，并受同一限速与重试约束。
- core：MTProto helper IPC 必须运行于 Tokio blocking pool，progress 通过异步通道回送原 upload future，避免同步 poll 边界重新串行化 upload worker。
- macOS UI：Endpoint Settings 增加 “Rate limit (advanced)” 编辑控件：
  - `max_concurrent_uploads`（1..8）
  - `min_delay_ms`（0..500）
  - 提示：过激参数可能触发 Telegram 限速，速度会被 `FLOOD_*_WAIT` 控制与自动退避。
- CI：增加 helper 的 `cargo test` 步骤（以及必要的 cache 覆盖），确保 helper 变更被门禁覆盖。

### Out of scope

- 配置 schema 版本升级（本次不新增字段、不改动语义，只完善实现使其更“有效”）。


## 需求（Requirements）

### MUST

- part size 必须满足 Telegram 官方约束：
  - `part_size % 1024 = 0`
  - `512KiB % part_size = 0`
  - 单 part 最大 512KiB
- 若启用多个 helper 进程并行上传，必须避免并发共享同一份 MTProto session/auth key 状态；应确保每个 helper 使用独立 session，且仅 primary helper 负责复用/更新持久化 session。
- helper 必须能解析并处理以下错误形式并按秒数退避重试：
  - `FLOOD_WAIT_<seconds>`
  - `FLOOD_PREMIUM_WAIT_<seconds>`
  - 以及已有的 `(... value: <seconds> ...)` 形式
- 任一并发 worker 收到 FloodWait 时，必须触发**全局冷却**，并确保所有并发分片在冷却期不会继续发送请求。
- 在冷却期（尤其是 >45s）必须持续输出 progress 心跳（建议间隔 <=10s），避免 core 侧 `MTPROTO_HELPER_UPLOAD_EVENT_TIMEOUT_SECS=45` 误判并重启 helper。
- CI 必须覆盖 helper 单测：`cd crates/mtproto-helper && cargo test`。

### SHOULD

- 在默认配置（例如 `min_delay_ms=250`）不变的情况下，仅通过 part size 提升，吞吐应有可观提升（预期同频率请求下接近 4x 数据量）。
- `max_concurrent_uploads` 是 direct、pack、index-part 与 index-manifest 合计的 Telegram file-part RPC 上限；core 使用同数 helper，且每个 document 只使用一个 file-part worker。
- UI 对非法范围做 clamped/blocked，并明确告知风险与回滚方式（恢复默认或下调并发/增大延迟）。
- 对 FloodWait parser、core 的 transient 分类与 flood-wait 检测补齐单测覆盖。

### COULD

- CI 增强缓存命中率（helper 的 `Cargo.lock` 与 target 缓存路径）。


## 功能与行为规格（Functional/Behavior Spec）

### Upload

- 默认 part size：512KiB。
- 并发（两层）：
  - core：通过共享自适应 slot、限速器与 helper pool，让 direct、pack、index-part upload job 分配到不同 helper 进程；slot id 是 run log 中的实际 worker 标识，在飞 document upload attempt 总数不超过 `max_concurrent_uploads`。
  - helper：每个 document 使用一个 file-part worker；core 同时调度至多 `max_concurrent_uploads` 个 document，使合计 file-part RPC 不超过该上限。
- index：分片在受限集合中并发上传；manifest 只在所有分片成功后进入相同的 slot 与限速器。
- 节流：每次 invoke 前遵循：
  1) 计算并等待 `next_allowed`（全局冷却/限流）。
  2) 发送请求。
  3) 若返回 FloodWait，解析 wait 秒数，更新全局 `next_allowed = max(next_allowed, now + wait)`，并重试。
- 心跳：若需要等待超过短时间阈值，应周期性输出 progress（以避免 core 超时）。
- session：多 helper 并行时，仅 primary helper 允许复用/更新持久化 session；secondary helpers 必须使用独立 session（避免 session 状态并发冲突）。

### Download

- 默认 part size：512KiB。
- 遇 FloodWait / FloodPremiumWait 同样走全局冷却，避免并发下载时继续撞限速。

### Edge cases / errors

- FloodWait 字符串大小写/格式差异：解析应尽可能健壮（例如包含其它上下文时也能提取秒数）。
- 极长等待：冷却期间不应 busy-loop；且 progress 心跳频率需控制（避免日志过量）。


## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `telegram_endpoints[].rate_limit.max_concurrent_uploads` | config | internal | Modify | None | core/helper | macOS UI, backup pipeline | 现有字段，仅 UI 暴露与实现更有效 |
| `telegram_endpoints[].rate_limit.min_delay_ms` | config | internal | Modify | None | core/helper | macOS UI, backup pipeline | 现有字段，仅 UI 暴露与实现更有效 |


## 验收标准（Acceptance Criteria）

- Given 默认配置与同一网络环境
  When 执行一次实际备份上传/下载
  Then 不出现 `FILE_PART_TOO_BIG` / `FILE_PART_SIZE_INVALID`，且吞吐相对 baseline 有明显提升。
- Given 返回 `FLOOD_WAIT_12` 或 `FLOOD_PREMIUM_WAIT_34`
  When helper 执行上传/下载
  Then 能解析秒数、全局冷却并在等待后自动继续，不会持续并发撞限速。
- Given `FLOOD_*_WAIT` 超过 45 秒
  When helper 进入等待
  Then core 不应因超时误判 helper 卡死（progress 心跳持续输出）。
- Given `max_concurrent_uploads > 1`
  When MTProto backend 上传一批对象（多个 upload job 并发排队）
  Then core 应通过 helper pool 实现并行上传，且不会因多 helper session 冲突导致永久卡死（progress 应持续推进）。
- Given `max_concurrent_uploads = 2`
  When direct、pack 或多个 index-part 具备上传工作
  Then `performance.upload` 所定义的 document RPC 最大并发为 2，data 与 index-part 均可观察到重叠，且限速与重试等待保持独立可见；每个 index attempt 记录实际 slot worker 与等待时长。
- Given CI
  When PR 触发 GitHub Actions
  Then helper tests 会被执行且全绿。
- Given macOS Endpoint Settings
  When 用户修改并保存 `max_concurrent_uploads` / `min_delay_ms`
  Then 配置被持久化，且 UI 提示清晰可回滚。


## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：
  - 过激并发/低延迟参数可能触发更频繁 FloodWait，导致抖动或更慢；需要通过默认安全值 + UI 提示缓解。
  - 等待阶段若缺少 progress 心跳，core 可能在 45s 后重启 helper，造成任务失败或重复工作。
- 多 helper 并行若错误地共享同一份 session/auth key 状态，可能出现卡死或请求异常；需要确保 session 隔离策略正确实现与覆盖测试。
  - 现有 worker 驱动中的同步 RPC 边界可能让配置的并发数退化为交替执行；M7 以实际
    RPC 重叠和全局上限作为验收，而非仅检查 worker 数量。
- 假设：
  - 主要瓶颈来自分片大小 + 节流（而非 scan/CPU）；以一次真实备份 run 的吞吐与日志确认。


## 参考（References）

- Telegram: [Uploading and Downloading Files](https://core.telegram.org/api/files)
- Telegram: [upload.getFile](https://core.telegram.org/method/upload.getFile)
