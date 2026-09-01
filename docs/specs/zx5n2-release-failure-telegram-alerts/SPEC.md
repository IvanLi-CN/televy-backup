# zx5n2 · Release 失败 Telegram 告警接入

## Summary
- 为 `Release` 工作流提供 repo-local notifier wrapper，复用固定版本的 Oidrune reusable workflow。
- 为失败发布保留真实 release target SHA 解析，确保告警能定位目标提交。
- 保留 `workflow_dispatch` smoke 路径，但真实 Telegram smoke 由维护者在具备通知授权时手动执行。

## Scope
- `.github/workflows/notify-release-failure.yml` 的失败发布和手动 smoke 调用。
- `.github/workflows/release.yml` 输出的 `RELEASE_REQUESTED_SHA` / `RELEASE_TARGET_SHA` 标记及其 resolver 消费。
- workflow contract tests 和本主题的实施记录。

## Caller Contract

- `workflow_run` 监听 `Release` 在 `main` 上的 `completed` 事件，只有结论为 `failure` 时通知。
- `workflow_dispatch` 保留手动 smoke 通知入口，使用当前 dispatch run 的 SHA 和 run URL。
- 两个调用 job 固定引用 `IvanLi-CN/oidrune/.github/workflows/notify.yml@e48822f99c6402a753ed86557ea029754cbab20b`。
- 两个调用 job 授予 `id-token: write`；调用方省略 `gateway_url` 与 `oidc_audience`，由 Oidrune 选择默认 gateway。
- 调用方不再传递 Telegram/Shoutrrr secret。
- 调用方必须完整生成 `summary`，至少包含：
-  - 首行为 `🚨 Release Failed · owner/repo`（failure）或 `🧪 Smoke Test · owner/repo`（smoke）；
  - 状态；
  - 目标 SHA；
  - run URL；
  - failure 或 smoke 标题。
- summary 保留多行正文格式，继续包含 workflow、event、ref、run attempt、actor 和 note。
- 失败路径额外保留 ref、run attempt、actor 和 target SHA 解析详情；smoke 路径额外保留 ref、run attempt、actor 和 smoke 说明。

## Non-goals

- 不改变 `Release` 的触发、失败判定、发布、tag 或 artifact 行为。
- 不在 CI contract test 中发送真实 Telegram 通知。
- 不让 repo-local caller 依赖 Oidrune 自动补充项目或运行元数据。

## Acceptance
- `workflow_run` 在 `Release` 失败时触发 Oidrune 通知。
- `workflow_dispatch` 保留手动 smoke test 通知路径。
- 两个调用均固定到经 live Oidrune facts 确认的完整 commit SHA。
- 两个调用均满足 OIDC permission、默认 gateway 和无旧 Telegram secret wiring 约束。
- caller-owned summary 包含完整项目、状态、目标 SHA、run URL 和 failure/smoke 标题。
- 失败告警优先携带真实 release target SHA，而不是仅回退到 workflow 头 SHA。
