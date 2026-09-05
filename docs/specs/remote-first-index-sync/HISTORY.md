# History

## Provenance

- Legacy source: `docs/plan/0012:remote-first-index-sync/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/architecture.md`：补充“remote-first index sync”的备份前置步骤与失败语义（实现阶段同步）。
- `README.md`：补充“重装/新设备继续备份”的推荐流程与开关说明（实现阶段同步）。


## Change log

- 2026-01-27: 实现 `index_sync` 备份前置对齐（remote-first），新增 `--no-remote-index-sync`，并完善 pinned catalog 异常路径（非 catalog 可覆盖；decrypt/parse 失败默认阻断 + 明确指引）。
- 2026-01-29: 远端索引下载后归一化 provider（跨设备 endpoint_id 不一致仍能复用去重/恢复），并避免 bootstrap decrypt 失败被误判为 missing。
- 2026-01-27: 针对私聊 `chat_id` 增加提前拦截与更明确的错误/告警（bootstrap 依赖 pinned，需群/频道或 `@username`）。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0012:remote-first-index-sync/PLAN.md`.
