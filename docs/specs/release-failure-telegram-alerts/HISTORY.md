# Release 失败 Telegram 告警接入演进历史

## Decision Trace

- 保留 repo-local wrapper 与 release target SHA resolver：项目需要保留现有 `Release` 失败判定和项目特有的 backfill target 解析，不能被通用通知模板覆盖。
- 从共享 GitHub workflow 切换到 Oidrune：通知认证与 gateway 选择由 Oidrune 统一处理，调用方只保留 outcome 和完整正文。
- 固定 Oidrune workflow commit：通知供应链必须可审计，避免使用可变 branch ref。
- 由 caller 生成完整 summary：Oidrune 的 `summary` 是必填且 caller-owned，项目上下文、失败标题、target SHA 和 run URL 不能依赖服务端推断。
- 保留手动 smoke 入口但不把真实通知发送纳入本地或 PR contract tests：真实 Telegram 是外部副作用，属于授权维护者的手动操作。

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
