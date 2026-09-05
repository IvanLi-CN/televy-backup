# History

## Provenance

- Legacy source: `docs/plan/0013:mtproto-dialogs-picker/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `README.md`：补充“如何选 chat_id（群/频道）”与 `telegram dialogs` 用法（如需）。


## Change log

- 2026-01-28: 发现 bot account 无法调用 `messages.getDialogs`（`BOT_METHOD_INVALID`）；方案调整为基于 update 流的 `wait-chat` 发现机制。
- 2026-01-28: 落地 `telegram wait-chat`（helper+core+CLI）与 macOS Settings “Listen…” picker；`cargo test` 通过；`./scripts/macos/build-app.sh` 通过。
- 2026-01-28: 修复 `get_pinned_message` 在“无 pinned message”场景触发 `MESSAGE_IDS_EMPTY` 导致备份失败；用隔离的 `target/e2e-*` 配置完成一次备份并通过 `restore list-latest` 验证 pinned catalog 可用。
- 2026-01-28: 修复“chat_id 切换后错误命中 dedupe”的根因：本地 `chunk_objects` 可能仍指向旧 peer（如私聊），导致 `bytes_uploaded=0` 但新频道为空；改为对 MTProto object_id 进行 peer 校验并在冲突时覆写映射（`ON CONFLICT(provider, chunk_hash) DO UPDATE`），确保后续会重新上传到当前频道。
- 2026-01-28: 修复状态面板上传速率/累计上传长期为 0：mtproto helper 输出 upload progress，core/daemon 透传并计算 `up.bytesPerSecond`/`upTotal.bytes`，用于 UI 实时展示。
- 2026-01-29: 端到端验证通过：重启 macOS app 并触发一次 daemon 备份，确认 UI 的 `Up`/`UpTotal` 在上传过程中实时更新（不再长期显示 0）。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0013:mtproto-dialogs-picker/PLAN.md`.
