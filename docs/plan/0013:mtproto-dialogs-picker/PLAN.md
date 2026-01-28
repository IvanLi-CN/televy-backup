# MTProto dialogs picker（自动选可用 chat_id）（#0013）

## 状态

- Status: 部分完成（3/4）
- Created: 2026-01-28
- Last: 2026-01-28

## 背景 / 问题陈述

- 当前 Telegram 存储是 MTProto-only，但用户容易误把 endpoint 的 `chat_id` 配成私聊（正数 id），导致 pinned bootstrap/catalog（依赖 pin）不可用，进而出现“明明可以备份，但跨设备 restore/latest/index-sync 不可用”的困惑。
- 目前 UI 需要用户手动填写 `chat_id`（或 `@username`），而用户往往不知道自己已加入的群/频道有哪些，也不知道如何查 channel/group 的 id。

结论：对 bot account 来说，Telegram 会拒绝 `messages.getDialogs`（无法直接列出完整 dialogs 列表）。因此需要提供一个“从 update 流里发现 chat（用户在目标群/频道发一条消息即可）并在 UI 里一键选择”的能力，避免手填与误配。

## 目标 / 非目标

### Goals

- 提供一个可靠的 chat_id 发现入口：在 bot 已加入的群/频道里发一条消息（必要时 @bot），CLI 能从 update 流解析出 chat，并给出可直接写入 `config.toml` 的 `chat_id` 值（优先 `@username`，否则用 numeric dialog id）。
- macOS Settings 里提供“Listen…”按钮：用户点击后开始监听（有限时），并把发现到的 **群/频道** 作为候选项，支持搜索/选择并自动填入 `chat_id`。
- 发现能力不依赖现有 `chat_id` 配置：即使 endpoint 的 `chat_id` 为空或无效，也能通过 update 流发现 chat 以便用户修复配置。
- 端到端验证：在一份可用的本机配置下，CLI 能输出 dialogs JSON，UI 能选中并写回设置。

### Non-goals

- 不新增/恢复 Telegram Bot API（仍为 MTProto-only）。
- 不实现“自动把 bot 拉进群/频道”或“自动建群/建频道”。
- 不实现群/频道权限诊断（例如是否具备 pin 权限），仅做基础提示（bootstrapHint）。

## 范围（Scope）

### In scope

- `televybackup telegram wait-chat`：输出 JSON（含 kind/title/username/peerId/configChatId/bootstrapHint）。
- MTProto helper：允许在 `chat_id` 为空时初始化并执行 `wait-chat`（不做 resolve_chat）。
- macOS app Settings：新增 picker UI（搜索 + 选择写回）。

### Out of scope

- daemon/UI 的大规模重构；仅做最小必要改动与错误提示。

## 需求（Requirements）

### MUST

- CLI 新增：`televybackup --json telegram wait-chat --endpoint-id <id> [--timeout-secs N] [--include-users]`
  - 输出结构稳定：`{ "chat": { ... } }`
  - `configChatId` 规则：若有 `@username` 则输出 `@username`；否则输出 numeric dialog id（可直接写入 config）。
  - `bootstrapHint`：group/channel 为 `true`；user 为 `false`。
- MTProto helper：当 `chat_id` 为空时仍可完成授权并执行 `wait-chat`；但 upload/download/pin/get_pinned 等必须返回清晰错误（提示需要配置 chat_id）。
- macOS Settings：在 endpoint 的 `chat_id` 输入旁新增 “Pick…”：
  - 点击后打开 picker，并允许 “Listen (60s)” 触发一次 CLI `telegram wait-chat`；
  - picker 默认只展示 `bootstrapHint=true`（群/频道）；
  - 支持搜索并选择，将 `configChatId` 写回 endpoint 的 `chat_id`。

### SHOULD

- 当当前 `chat_id` 看起来是私聊（正数 id）时，UI/CLI 给出明确提示：bootstrap/pin 不支持或不可靠，建议改为群/频道或 `@username`。

## 验收标准（Acceptance Criteria）

- Given endpoint secrets（bot token + api_hash）可用，
  When 运行 `televybackup --json telegram wait-chat --endpoint-id <id> --timeout-secs 60`，并在目标群/频道发送一条消息，
  Then 输出 chat 信息，且 command 在合理时间内完成（不无限卡住）。

- Given 用户在 macOS Settings 打开某 endpoint，
  When 点击 “Listen…” 并通过 “Listen (60s)” 发现一个群/频道条目后选择它，
  Then `chat_id` 自动填充为可用值（`@username` 或 numeric），并保存到设置中。

## 非功能性验收 / 质量门槛（Quality Gates）

- dialogs 列表需有整体超时/错误提示，避免 UI 永久等待。
- 不把运行/交付依赖挂在 `docs/plan/` 下。

## 文档更新（Docs to Update）

- `README.md`：补充“如何选 chat_id（群/频道）”与 `telegram dialogs` 用法（如需）。

## 实现里程碑（Milestones）

- [x] M1: MTProto helper 支持 chat-less init + wait-chat 稳定输出（含超时）
- [x] M2: CLI `telegram wait-chat` 可用（chat_id 允许为空）
- [x] M3: macOS Settings 增加 “Listen…” picker 并能写回 chat_id
- [ ] M4: 端到端验证（CLI + UI）

## Change log

- 2026-01-28: 发现 bot account 无法调用 `messages.getDialogs`（`BOT_METHOD_INVALID`）；方案调整为基于 update 流的 `wait-chat` 发现机制。
- 2026-01-28: 落地 `telegram wait-chat`（helper+core+CLI）与 macOS Settings “Listen…” picker；`cargo test` 通过；`./scripts/macos/build-app.sh` 通过。
- 2026-01-28: 修复 `get_pinned_message` 在“无 pinned message”场景触发 `MESSAGE_IDS_EMPTY` 导致备份失败；用隔离的 `target/e2e-*` 配置完成一次备份并通过 `restore list-latest` 验证 pinned catalog 可用。
- 2026-01-28: 修复“chat_id 切换后错误命中 dedupe”的根因：本地 `chunk_objects` 可能仍指向旧 peer（如私聊），导致 `bytes_uploaded=0` 但新频道为空；改为对 MTProto object_id 进行 peer 校验并在冲突时覆写映射（`ON CONFLICT(provider, chunk_hash) DO UPDATE`），确保后续会重新上传到当前频道。
