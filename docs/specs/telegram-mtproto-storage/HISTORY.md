# History

## Provenance

- Legacy source: `docs/plan/0004:telegram-mtproto-storage/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/architecture.md`: 补充 MTProto 存储模型与对象引用刷新策略的约束。
- `docs/requirements.md`: 补充 MTProto 模式配置与故障排查（validate/重登/刷新引用）。
- `README.md`: 补充 MTProto 模式启用步骤、风险提示与迁移说明（Bot API 不再支持）。


## 方案概述（Approach, high-level）

- 以 `Storage` 抽象为边界：仅保留 MTProto 存储实现，备份语义与加密/索引逻辑保持不变。
- `object_id` 采用“可版本化 + 可恢复定位”的编码：需要能在 file reference 过期时，通过稳定定位信息刷新引用。
- 下载路径以“分片 + 可续传”为主，避免全量内存占用；恢复时按既有 framing 解密与落盘校验。
- 统一错误分类与脱敏：对鉴权失效、对象不存在、引用过期、网络抖动、限速等给出可操作提示。


## 变更记录（Change log）

- 2026-01-21：口径变更：**MTProto-only**，移除 Telegram Bot API 全链路；配置 `telegram.mode` 固定 `mtproto`；历史 `telegram.botapi` snapshot/provider 标记为不受支持（需重新备份）。
- 2026-01-21：落地 `telegram.mode=mtproto`：新增 `tgmtproto:v1` object_id（Base64URL JSON），引入本地加密 secrets store（`secrets.enc` + Keychain vault key），并通过独立 `televybackup-mtproto-helper`（避免 sqlite links 冲突）实现可续传下载；CLI/daemon 支持 mtproto validate + session 持久化与 provider mismatch 可操作报错。
- 2026-01-21：完成 M6：移除 Bot API 相关实现与文档；旧 `telegram.mode=botapi` 配置自动迁移为 `mtproto`（不强制补齐 `api_id/api_hash`）；更新 `docs/architecture.md`、`docs/requirements.md`、`README.md`。
- 2026-01-22：修复 MTProto helper 对数字 chat_id 的解析：避免使用 bots 不允许的 `messages.getDialogs`；补齐 sender pool runner 驱动；修复 macOS case-insensitive 文件系统下 CLI/APP 同名导致的反复启动问题；精简 GUI（移除无意义的 Copy/Testing 文案）。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0004:telegram-mtproto-storage/PLAN.md`.
