# Telegram 通信升级为 MTProto API（MTProto-only，移除 Bot API）（#0004）

## 状态

- Status: 已完成
- Created: 2026-01-20
- Last: 2026-01-22

## 已冻结决策（Decisions, frozen）

- MTProto 选型：纯 Rust MTProto 客户端库（不使用 TDLib）。
- MTProto crate：优先使用 `grammers-client`（crates.io 依赖；按 semver 锁定到 `0.8.*`，并由
  `Cargo.lock` 固定最终版本；避免 git 依赖）。
- `object_id` 编码：采用 `tgmtproto:v1:<base64url(json)>`（见 `./contracts/db.md`）；允许被 `tgpack:` 包裹用于 pack slice。
- 旧数据兼容：不要求兼容/恢复历史 Bot API 备份数据；当 snapshot/provider 为 `telegram.botapi` 时必须给出可操作错误提示（重新备份）。
- secrets 存储：Keychain **只存 1 条** vault key（`televybackup.vault_key`）；其余敏感信息写入本地加密
  secrets store（`secrets.enc`，见 `./contracts/file-formats.md`）。并将历史 Keychain secrets（bot token /
  master key）迁移到 secrets store 后删除旧 Keychain items。
- 限速与并发：复用现有 `telegram.rate_limit` 作为 upload+download 的统一并发/节流配置（不引入
  `mtproto` 专用字段；如未来需要再另立契约变更）。
- 通信模式：**MTProto-only**。移除 Telegram Bot API 存储实现与相关配置分支；配置 `telegram.mode`
  固定为 `mtproto`。

## 背景 / 问题陈述

- 当前存储层基于 Telegram Bot API：上传走 `sendDocument`，下载走 `getFile` + `file/bot...`。
- Bot API 的下载能力存在体积上限：当远端对象较大（例如 pack/chunk/index part）时，`restore/verify` 的“取回 → 解密 → 落盘 → 校验”链路会失败，导致备份不可恢复或不可验证。
- 需要引入 Telegram MTProto API 的传输能力：支持大对象上传/下载，并在失败时可重试、可续传、可定位原因。
- 口径变更：移除 Bot API 支持；历史 `telegram.botapi` snapshot/provider 视为不受支持（需重新备份）。

## 用户与场景（Users & Scenarios）

- 单用户 macOS：备份指定目录到 Telegram；任意时刻可从 Telegram 取回并恢复到指定目录，并能通过校验。
- 典型路径：
  - backup：scan → chunk → hash → encrypt → upload → index
  - restore/verify：index → download → decrypt → write → hash/size 校验

## 目标 / 非目标

### Goals

- 支持明显超过 Bot API 下载上限的远端对象下载，并完成 “取回 → 解密 → 落盘 → 校验” 全链路。
- 不改变既有备份语义：chunking/hash/加密 framing/SQLite 索引逻辑保持一致（仅替换通信层与对象引用格式）。
- 口径变更：**MTProto-only**；移除 Bot API 并存。
- 可观测性：任务进度、失败原因、重试行为在 CLI/GUI 可追踪；日志与事件流默认脱敏。

### Non-goals

- 不内置/不实现自建 `telegram-bot-api`（Local Bot API Server）作为服务端组件。
- 不引入多设备多用户共享、群组/频道存储、跨账号同步等扩展。
- 不改变加密算法与 chunking 策略（除非为通信适配增加必要元数据字段）。
- 旧数据“批量迁移/重建引用”的工具不作为本计划交付范围（如需要另立计划）。

## 范围（Scope）

### In scope

- Rust core：新增 `telegram.mtproto` 存储适配器（对齐现有 `Storage` 抽象：`upload_document` / `download_document`）。
  - 入口位置（待实现）：`crates/core/src/storage.rs`（可能拆分为 `storage/telegram_mtproto.rs`）。
  - 代码触点（recon）：
    - `crates/core/src/storage.rs`：新增 provider + `tgmtproto` 解析/上传/下载
    - `crates/core/src/backup.rs`：写入 `object_id`/provider；上传 index parts/manifest
    - `crates/core/src/restore.rs`：按 provider 拉取对象；pack slice 解包；provider mismatch 报错口径
- 鉴权与会话持久化：
  - 支持以 bot 身份登录 MTProto（基于 bot token 导入授权）；
  - 会话可持久化，重启后可继续使用；
  - bot token、MTProto 会话、以及必要的 API 凭据不落入仓库与明文配置文件：
    - Keychain 仅保存 1 条 vault key（`televybackup.vault_key`）
    - 其余 secrets 写入本地加密 secrets store（`secrets.enc`）
    - 迁移：把历史 Keychain secrets（bot token / master key）迁移至 secrets store 并删除旧 items
- 上传：
  - 上传对象到目标对话（与现有 `telegram.chat_id` 语义一致）；
  - 返回可持久化的 `object_id`（支持版本化，例如 `tgmtproto:v1:...`），后续可可靠定位到远端文件；
  - 支持 “引用刷新/过期处理” 的可恢复策略（见 `./contracts/db.md`）。
- 下载（大对象）：
  - 给定 `object_id`，可下载大对象并返回字节流；
  - 支持分片下载与断点续传：失败重试不重复下载已完成分片（含进程退出后重试；本地续传状态存放在
    `$TELEVYBACKUP_DATA_DIR/cache/` 下的 mtproto 下载缓存）。
  - 性能：避免无谓全量内存拷贝；下载落盘路径以流式为主（restore/verify 可控内存峰值）。
- SQLite index 兼容：
  - `chunk_objects.object_id`、`remote_indexes.manifest_object_id`、`remote_index_parts.object_id` 支持持久化 MTProto 引用；
- 不要求兼容读取既有 Bot API 备份数据；若 snapshot/provider 为 `telegram.botapi` 必须给出可操作提示（重新备份）。
- CLI：
  - `televybackup telegram validate` 在 `telegram.mode=mtproto` 下执行 “已登录/可访问 chat/可上传下载测试对象且内容一致” 的验证流程（见 `./contracts/cli.md`）。
  - 代码触点（recon）：`crates/cli/src/main.rs`（config/validate/secrets store/vault）
- Daemon：
  - MTProto-only：执行 scheduled backup 时仅使用 `telegram.mtproto`（其余模式/旧配置仅做迁移与报错提示）。
  - 代码触点（recon）：`crates/daemon/src/main.rs`
- macOS GUI：
  - Settings 中展示 MTProto 连接与验证状态（成功/失败/原因），并确保日志脱敏（具体 UI 细节由实现阶段对齐 `macos/TelevyBackupApp` 现状）。
  - 代码触点（recon）：`macos/TelevyBackupApp/TelevyBackupApp.swift`

### Out of scope

- 与计划 #0005（多 endpoint + 多 target）进行 schema 合并/重构：本计划先以当前 v1 配置形状落地（单 endpoint），并保证后续可迁移。
- 在 Telegram 远端聊天历史中“全量搜索 manifest”作为恢复入口（仍以本地 SQLite 为主；远端可发现性由计划 #0005 负责）。

## 需求（Requirements）

### MUST

- 新增 MTProto 存储适配器：
  - provider: `telegram.mtproto`
  - MTProto-only：不提供 Bot API 行为分支；历史配置会被自动迁移为 `mtproto`。
- 鉴权与会话持久化：
  - 支持以 bot 身份登录；
  - 会话持久化（重启后可继续使用）；
  - secrets 不得写入仓库与明文配置文件；日志/事件流必须脱敏。
- 上传：
  - 上传对象到目标对话并返回可持久化 `object_id`；
  - `object_id` 自描述且可版本化；并能在“引用过期/刷新”时仍可恢复定位。
- 下载（大对象）：
  - `object_id` → 下载字节流；
  - 支持分片下载/断点续传；失败重试不会重复下载已完成分片（含进程退出后重试）；
  - 失败时返回可分类的错误（可重试/不可重试）并给出可操作建议。
- 对象引用格式与 DB 兼容：
  - `tgmtproto:v1:` 前缀编码写入（具体形状见 `./contracts/db.md`）；
  - pack slice：使用 `tgpack:<pack_object_id>@<offset>+<len>`，其中 `pack_object_id` 允许为 `tgmtproto:v1:...`；
  - 对未知前缀给出明确错误提示（例如需要升级版本或 `object_id` 损坏）。
  - 不支持恢复历史 `telegram.botapi` snapshot（需给出可操作提示：重新备份）。
- 配置与验证：
  - 新增 `telegram.mode=mtproto` 与必要的 MTProto 配置/secret 入口（见 `./contracts/config.md`）；
  - CLI `telegram validate` 在 mtproto 模式下完成一次端到端回环校验（上传→下载→内容一致）。
- 限速与并发：
  - MTProto 上传/下载必须受并发与节流控制（沿用现有 `telegram.rate_limit` 或新增 `telegram.mtproto.rate_limit`，但需保持配置风格一致）。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `config.toml` → `[telegram]`（新增 `mode=mtproto` 与 mtproto 配置/secret key） | Config | internal | Modify | `./contracts/config.md` | CLI/Core/GUI | CLI/daemon/GUI | v1 形状；需保证后续可迁移到 v2 |
| 本地 secrets store（`secrets.enc`）+ vault key（Keychain 仅 1 item） | File format | internal | New | `./contracts/file-formats.md` | CLI/Core/GUI | CLI/daemon/GUI | 迁移：旧 Keychain secrets → secrets store |
| `televybackup telegram validate`（mtproto 端到端验证） | CLI | internal | Modify | `./contracts/cli.md` | CLI | GUI/User | MTProto-only：完成“上传→下载→比对一致”的回环校验 |
| SQLite object_id（新增 `tgmtproto:` 编码与兼容策略） | DB | internal | Modify | `./contracts/db.md` | Core | Core/CLI/GUI | 必须支持版本化与可恢复定位 |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/config.md](./contracts/config.md)
- [contracts/cli.md](./contracts/cli.md)
- [contracts/db.md](./contracts/db.md)
- [contracts/file-formats.md](./contracts/file-formats.md)

## 验收标准（Acceptance Criteria）

- Given `telegram.mode=mtproto` 且已配置必要凭据与目标对话，
  When 运行 `televybackup telegram validate`，
  Then 返回验证成功，并完成一次“上传→下载→比对一致”的回环。
- Given Telegram 上存在单个明显超过 Bot API 下载上限的备份对象，
  When 执行 `restore` 到指定目录，
  Then 能完整取回、解密落盘并通过校验（hash/大小/文件数一致）。
- Given 下载过程中断（断网/进程退出），
  When 重试 `restore`，
  Then 能继续下载并最终成功，且不会重复下载已完成分片（可从日志/统计验证）。
- Given 凭据失效或引用过期，
  When 执行 `download/restore/verify`，
  Then 给出明确错误码与可执行的修复动作说明（重登/刷新引用/重新验证等）。
- Given 从旧版本升级且 bot token/master key 已存在于 Keychain（旧 scheme），
  When 执行 `televybackup secrets migrate-keychain`，
  Then secrets store 创建成功、secrets 被迁移，旧 Keychain items 被删除（仅保留 `televybackup.vault_key`），
  且后续 `telegram validate` / `backup` / `restore` 不需要重新输入 token/master key。

## 实现前置条件（Definition of Ready / Preconditions）

None

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests:
  - `tgmtproto:` object_id 的解析/序列化与版本化（包含“不得包含 `@`/`+` 等 pack slice 分隔符”的约束）。
  - 分片下载断点续传的正确性（使用 mock storage 或可控 stub；不依赖真实 Telegram）。
  - 错误分类与重试策略（可重试/不可重试）与脱敏规则（token/session 不出现在日志/事件中）。
- Integration tests:
  - 在不访问真实 Telegram 的前提下跑一次最小 restore/verify 流程（使用 in-memory storage 或 stub）。
  - 手工验收：`telegram validate` + 真实大对象 restore（作为 release 前 checklist，不自动化进 CI）。

### Quality checks

- `cargo fmt`, `cargo clippy -- -D warnings`, `cargo test`（不引入新工具）。

## 文档更新（Docs to Update）

- `docs/architecture.md`: 补充 MTProto 存储模型与对象引用刷新策略的约束。
- `docs/requirements.md`: 补充 MTProto 模式配置与故障排查（validate/重登/刷新引用）。
- `README.md`: 补充 MTProto 模式启用步骤、风险提示与迁移说明（Bot API 不再支持）。

## 实现里程碑（Milestones）

- [x] M1: 冻结具体 MTProto crate + session 持久化方案（并在契约中固化）
- [x] M2: `telegram.mtproto` 基础连通（bot 登录 + upload_document + 小对象 download_document）
- [x] M3: 大对象下载（分片/续传/重试 + 节流/并发控制 + 低内存峰值）
- [x] M4: DB 口径与 `object_id` 形状落地（`tgmtproto:v1:` + provider 不匹配错误提示）
- [x] M5: CLI validate + GUI 状态展示 + 文档与测试补齐
- [x] M6: MTProto-only：移除 Bot API 全链路（core/cli/daemon/gui/docs）

## 方案概述（Approach, high-level）

- 以 `Storage` 抽象为边界：仅保留 MTProto 存储实现，备份语义与加密/索引逻辑保持不变。
- `object_id` 采用“可版本化 + 可恢复定位”的编码：需要能在 file reference 过期时，通过稳定定位信息刷新引用。
- 下载路径以“分片 + 可续传”为主，避免全量内存占用；恢复时按既有 framing 解密与落盘校验。
- 统一错误分类与脱敏：对鉴权失效、对象不存在、引用过期、网络抖动、限速等给出可操作提示。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

### 风险

- MTProto 生态依赖与升级成本：不同库的 API/稳定性差异大，可能影响维护与打包分发。
- `object_id` 设计失误会导致“不可恢复”：必须优先冻结“稳定定位 + 引用刷新”的策略。
- 限速与风控：MTProto 的并发/请求模式可能触发风控，需要保守默认值与可调节策略。

### 开放问题（需要主人决策）

None

### 假设（需主人确认）

None

## 变更记录（Change log）

- 2026-01-21：口径变更：**MTProto-only**，移除 Telegram Bot API 全链路；配置 `telegram.mode` 固定 `mtproto`；历史 `telegram.botapi` snapshot/provider 标记为不受支持（需重新备份）。
- 2026-01-21：落地 `telegram.mode=mtproto`：新增 `tgmtproto:v1` object_id（Base64URL JSON），引入本地加密 secrets store（`secrets.enc` + Keychain vault key），并通过独立 `televybackup-mtproto-helper`（避免 sqlite links 冲突）实现可续传下载；CLI/daemon 支持 mtproto validate + session 持久化与 provider mismatch 可操作报错。
- 2026-01-21：完成 M6：移除 Bot API 相关实现与文档；旧 `telegram.mode=botapi` 配置自动迁移为 `mtproto`（不强制补齐 `api_id/api_hash`）；更新 `docs/architecture.md`、`docs/requirements.md`、`README.md`。
- 2026-01-22：修复 MTProto helper 对数字 chat_id 的解析：避免使用 bots 不允许的 `messages.getDialogs`；补齐 sender pool runner 驱动；修复 macOS case-insensitive 文件系统下 CLI/APP 同名导致的反复启动问题；精简 GUI（移除无意义的 Copy/Testing 文案）。

## 参考（References）

- Telegram MTProto: https://core.telegram.org/mtproto
- grammers（纯 Rust Telegram/MTProto 客户端库，MIT/Apache-2.0）：
  - 概览与许可证：https://raw.githubusercontent.com/Lonami/grammers/master/README.md
  - `grammers-client` crate 文档（支持 bot 账号、替代 Bot API）：https://docs.rs/grammers-client
  - 认证（包含 `Client::bot_sign_in`）：https://docs.rs/crate/grammers-client/0.8.1/source/src/client/auth.rs
  - 文件上传/下载（含 `GetFile`/`SaveFilePart`/`SaveBigFilePart`）：https://docs.rs/crate/grammers-client/0.8.1/source/src/client/files.rs
