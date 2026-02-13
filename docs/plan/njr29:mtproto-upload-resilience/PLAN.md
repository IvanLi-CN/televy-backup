# MTProto upload resilience（retry + heartbeat）（#njr29）

## 状态

- Status: 待实现
- Created: 2026-02-13
- Last: 2026-02-13

## 背景 / 问题陈述

- 正式版在执行 backup 时反复失败，UI 显示 `err=telegram.unavailable`。
- 任务日志显示失败发生在 upload 阶段（扫描与打包完成后）：
  - `save_file_part timed out after 60s`
  - `save_big_file_part timed out after 60s`
  - 极端情况下：core 等待 helper 响应超过 180s 后 kill helper：`mtproto helper timed out waiting for response after 180s`
- 现状对用户的可用性影响：一旦遇到 Telegram 侧短暂卡顿/抖动，整轮 backup 直接失败；且偶发“长时间 Running 卡住”。

## 目标 / 非目标

### Goals

- MTProto upload 分片（`save_file_part` / `save_big_file_part`）在超时或可判定为暂时性错误时自动重试（带 backoff）。
- 在 upload 阶段出现“短时间无进度”时，helper 仍应周期性输出 `upload_progress`（heartbeat），避免 core 误判 helper 无响应。
- 错误信息可诊断：失败时携带 “哪一步/第几次尝试/累计尝试数”。

### Non-goals

- 不引入代理/翻墙能力（SOCKS/HTTP proxy）与相关 UI 配置。
- 不更改 Telegram endpoint/targets 的业务语义与配置格式。

## 范围（Scope）

### In scope

- `crates/mtproto-helper`：
  - 为 `save_file_part` / `save_big_file_part` 增加 retry + backoff（有限次数、可中止）。
  - upload 期间增加 heartbeat 输出（即使 `bytesUploaded` 未变化，也保证在固定上限间隔内输出）。
- `crates/core`：
  - 不需要调整协议；只要 helper 输出仍为合法 JSON line（ResponseEnvelope）即可。

### Out of scope

- 修改 pack/chunking 策略、上传并发策略（除非为实现 retry 必要的最小调整）。
- 调整 daemon 调度/任务编排。

## 验收标准（Acceptance Criteria）

- 当网络短暂抖动导致单个分片请求超时（例如 60s 内无响应）时：
  - helper 自动重试，若后续恢复则整轮 backup 能继续并最终成功（不再必然失败）。
- 当网络持续不可用时：
  - backup 仍会失败，但错误信息包含：`save_*_part`、attempt x/y、以及最后一次错误原因。
- 当 upload 阶段出现无进度窗口时：
  - core 不应再因为 180s 无输出而 kill helper（heartbeat 保证有输出）。

## Testing

- 单元测试覆盖：
  - retry/backoff 的错误分类与重试边界（例如 timeout / flood wait）。
- 本地最小验证：
  - `cargo test -p mtproto-helper`

## Milestones

1) 为 `save_file_part` / `save_big_file_part` 实装 retry + backoff（含 flood wait 支持）。
2) upload 阶段增加 `upload_progress` heartbeat（无进度也输出）。
3) 补齐与 retry/heartbeat 相关的单元测试。

## 风险与开放问题

- 重试会拉长失败路径耗时；需要通过“最大尝试次数 + 最大 backoff”限制上界。
- heartbeat 会增加 stdout 行数与日志量，但可显著提升可观测性并避免误杀。

