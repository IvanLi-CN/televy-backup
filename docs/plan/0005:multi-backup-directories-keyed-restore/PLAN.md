# 设置窗口独立化与多备份目录（多 Telegram Endpoint + 金钥恢复）（#0005）

## 状态

- Status: 待设计
- Created: 2026-01-20
- Last: 2026-01-21

## 已冻结决策（Decisions, frozen）

- bootstrap/catalog 可发现方式：使用 Telegram chat 的 **pinned message** 作为 root pointer（被 pin 的消息携带加密的 catalog document）。
- backup target 粒度：**一个目录一个 target**（单 `source_path`）。
- 调度（schedule）：每个 target 有独立 schedule；默认继承全局 schedule（可按字段 override）。
- 金钥（recovery key）：以**字符串**形式导入/导出；允许在 Settings window 内展示与复制（默认隐藏，显式 reveal）。
- endpoint 标识：使用稳定的 `endpoint_id`（不可变；用于 provider namespace），多个 target 可复用同一 endpoint。
- UI 图标：使用 Iconify 图标（本计划设计图采用 `tabler:*`），便于实现阶段与设计保持一致。
- secrets 存储：对齐计划 #0004：Keychain **只存 1 条** vault key（`televybackup.vault_key`）；其余敏感信息
  写入本地加密 secrets store（`secrets.enc`）。

## 背景 / 问题陈述

- 当前 macOS App 的设置（Settings）是 Popover 内的一个 Tab：不符合“系统设置/Preferences”的常见交互，且后续设置项会继续膨胀，维护与可用性会变差。
- 当前配置对 Telegram 存储目标只有一个全局 `telegram`（bot token + chat_id），无法表达“多个备份目录分别对应不同 Telegram endpoint（bot/chat）”，也无法在 UI 中自然地管理多个备份条目。
- 当前恢复（restore/verify）依赖本地 SQLite index 中的 `manifest_object_id`，缺少“新设备在没有旧 SQLite 的前提下，从 Telegram 端自助发现并完成恢复”的路径（`docs/architecture.md` 已标注该限制）。
- 本计划希望在不引入新后端的前提下：拆出独立 Settings window，支持多备份目录与 endpoint 绑定，并提供可迁移的“金钥（recovery key）”与可发现的远端引导信息，使“换电脑/重装后仍可下载并解密恢复”。

## 用户与场景（Users & Scenarios）

**用户**

- 单用户 macOS：可能有多个需要备份/恢复的目录（多个外接盘、多个路径、或多个备份盘挂载点）。

**核心场景**

- 用户在 Settings window 中配置多个 backup target，并为每个 target 选择对应的 Telegram endpoint（bot token + chat_id）。
- 用户在新电脑上重装后，仅凭“金钥 + Bot token + chat_id”（以及要恢复的目录/target 选择）即可完成 restore/verify。

## 目标 / 非目标

### Goals

- 新增独立 Settings window（标准 macOS Preferences 样式：titlebar toolbar + 系统表单/列表控件），作为**主设置入口**，支持编辑全部设置项与列表型配置。
- Popover 的 Settings tab 保持现有视觉与结构，**仅做最小必要改动**（例如增加打开 Settings window 的入口；必要时提供只读摘要/快捷操作）。
- 支持配置多个“backup target（备份条目）”，每个条目至少包含：`source_path`、`label`（可选）、绑定的 Telegram endpoint 引用。
- 每个 backup target 的 **Bot 信息（bot token + chat_id）必须在该 target 的编辑界面中直接可见/可编辑**（允许选择/复用已有 `endpoint_id`，但不强制用户先去单独页面配置 endpoint）。
- 支持配置多个 Telegram endpoint（bot token + chat_id），且允许多个 backup target 复用同一个 endpoint。
- 支持“金钥”导入/导出：允许把 master key 从旧设备迁移到新设备（存于 secrets store；`config.toml` 不落 secret）。
- 支持跨设备恢复：在没有旧设备本地 SQLite 的前提下，能够从 Telegram 获取所需的 `snapshot_id + manifest_object_id` 并完成 restore/verify。
- 多 endpoint 场景下必须有稳定的 provider namespace 规则：避免不同 endpoint 的对象引用/去重互相污染，且 namespace 不包含 token 明文。

### Non-goals

- 不做多窗口复杂应用（仅新增 Settings window；Popover 仍保留核心状态/触发入口）。
- 不在本计划内实现 Telegram 远端 GC/聊天历史清理策略。
- 不在本计划内引入新的存储后端（S3/WebDAV 等）。
- 不在本计划内实现“无需任何 Telegram 能力的离线恢复”（仍需 bot token 访问 Telegram）。

## 范围（Scope）

### In scope

- macOS App：Settings window 结构与交互；backup target 列表的新增/删除/编辑；endpoint 管理入口；金钥导入/导出入口（或引导到 CLI）。
- CLI：
  - settings schema v2 的读取/校验与写回；
  - 多 endpoint 的 secret presence（每个 endpoint 的 token 是否已写入 secrets store）；
  - 金钥导入/导出；
  - 恢复命令新增“按 source/target 恢复”的用户路径（无需手填 snapshot_id/manifest_object_id）。
- Daemon：按配置运行多个 backup target（可能跨 endpoint）；并为每个 endpoint 维护远端 bootstrap/catalog（可发现的引导信息）。
- Core：为多 endpoint 透传稳定的 provider namespace（不含 secret）；以及 bootstrap/catalog 的上传/下载辅助能力。

### Out of scope

- 旧配置/旧数据的自动无损迁移到不同 chat（如果用户想换 chat，视为新 endpoint）。
- 复杂权限/多用户隔离。

## 需求（Requirements）

### MUST

- Settings window
  - 必须存在独立 Settings window（非 Popover 内 tab），可从 App 菜单/Popover 进入，且符合 macOS 常见“Preferences/Settings”体验。
  - 必须支持在 Settings 中新增/删除/编辑多个 backup target。
  - 必须在“目录（target）编辑”界面中直接完成该目录对应的 Bot 配置：
    - Chat ID 可编辑
    - Bot token 可写入/更新 secrets store（token 不通过 argv；从输入框/安全输入写入）
    - 可对该目录对应的 bot/chat 进行 `validate`（含 pinned bootstrap 的可用性）
- Config & mapping
  - 必须支持多个 backup target，每个 target 必须能绑定一个 Telegram endpoint（bot token + chat_id 的组合）。
  - 必须允许多个 target 绑定同一个 endpoint。
  - 必须保持“不在 config.toml 中存 token 明文”的约束；token 仅存 secrets store（加密）。
- Recovery / portability
  - 必须提供“金钥”导入/导出能力，使得用户可在新设备重建解密能力（master key）。
  - 必须定义并实现一个“远端引导信息（bootstrap/catalog）”机制，使得在新设备没有旧 SQLite 的情况下，也能定位到要恢复的 snapshot（至少 latest）所需的 `manifest_object_id`。
  - 必须保证用户只需要：金钥 + bot token + chat_id（以及要恢复的 source_path/target 选择）即可完成 restore/verify。
- Safety & privacy
  - 必须明确并实现 provider namespace 规则：用于区分不同 endpoint 的对象引用/去重；namespace 不得包含 token 明文。
  - 必须在 UI/CLI 输出中避免泄露 secrets（token/master key/金钥本体）。
- Backward compatibility
  - 必须为现有单 endpoint 配置提供兼容策略（读取 v1；保存/写回时迁移到 v2，或提供一次性迁移命令）。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `config.toml` schema v2 | Config | internal | Modify | ./contracts/config.md | core/cli | app/daemon | targets + endpoints + 兼容 v1 |
| CLI/IPC commands | CLI | internal | Modify | ./contracts/cli.md | cli | app/daemon | settings/secrets/restore 的形状变化 |
| Remote bootstrap/catalog | File format | internal | New | ./contracts/file-formats.md | core/daemon | cli/app | Telegram 端可发现的引导信息 |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/config.md](./contracts/config.md)
- [contracts/cli.md](./contracts/cli.md)
- [contracts/file-formats.md](./contracts/file-formats.md)

## 验收标准（Acceptance Criteria）

- Given TelevyBackup App 运行，
  When 用户点击 “Settings…”（或等价入口），
  Then 打开独立 Settings window；Popover 的 Settings tab 仍保持原有信息架构与视觉风格，仅新增一个打开 Settings window 的入口。
- Given 配置中存在 2 个 backup target，
  When 用户在 Settings window 查看与编辑，
  Then 可以新增/删除并保存，且 CLI `settings get --json` 返回包含这 2 个 target 的结构化数据。
- Given 用户选中某个目录 target，
  When 在该目录的编辑界面输入/更新 Chat ID 与 bot token 并点击 Test connection，
  Then `telegram validate --endpoint-id <endpoint_id>` 成功（或返回可操作的错误），且 bot token 不会写入 `config.toml`。
- Given 两个 backup target 引用同一 endpoint，
  When 分别触发 backup，
  Then 二者均能正常上传/更新索引，且不会要求重复录入 bot token。
- Given 旧设备已至少完成一次备份并已生成远端 bootstrap/catalog，
  When 新设备导入金钥并配置相同 endpoint（bot token + chat_id），
  Then 能列出可恢复的 target（至少 latest），并可恢复到指定目录且 verify 通过。
- Given endpoint 的 chat 不可用（用户删号/ bot 被 block/ bot 被踢出群等），
  When 执行 backup 或恢复，
  Then 返回稳定错误码与用户可操作的提示（例如需更换 endpoint/重新 init bootstrap）。

## 实现前置条件（Definition of Ready / Preconditions）

- 目标/非目标、范围（in/out）、约束已明确
- 远端 bootstrap/catalog 的“可发现方式”已由主人确认（例如 pinned message vs 其他机制）
- config schema v2 与 provider namespace 规则已冻结（或明确迁移策略）
- 金钥导入/导出 UX（仅 CLI vs UI 也提供入口）已由主人确认
- CLI 恢复入口（按 source/target vs 按 snapshot_id）已冻结

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests:
  - config schema v2 parse/validate（含 v1 兼容读取）
  - provider namespace 生成规则稳定性
  - 金钥导入/导出（长度/格式校验；不覆盖已有 key 的策略）
  - bootstrap/catalog JSON schema + 加解密 round-trip（使用 InMemoryStorage）
- Integration tests:
  - 使用 InMemoryStorage 跑一轮 backup → 生成 bootstrap/catalog → 在“空 data_dir”场景下完成 restore/verify

### Quality checks

- 按仓库既有约定执行 fmt/lint/test（不引入新工具）。

## 文档更新（Docs to Update）

- `docs/architecture.md`：更新 Known limitations（新增跨设备 restore 机制），补充 bootstrap/catalog 与多 endpoint 设计。
- `docs/plan/0001:telegram-backup-mvp/contracts/file-formats.md`：同步 config schema（v1 → v2）与 bootstrap/catalog 说明。
- `README.md`：增加“金钥备份/迁移”与“新设备恢复”指引。

## 实现里程碑（Milestones）

- [ ] M1: `config.toml` schema v2（targets + endpoints + per-target schedule）与 v1 兼容读取/迁移写回
- [ ] M2: provider namespace 变更：`telegram.botapi/<endpoint_id>`（多 endpoint 去重隔离）
- [ ] M3: multi-endpoint secrets + validate：每个 endpoint 的 token 写入 secrets store、按 endpoint validate
- [ ] M4: bootstrap/catalog：加密 catalog 文档上传 + pin root pointer + resolve latest（供 restore/verify 使用）
- [ ] M5: restore/verify 新入口：`latest`（按 `target_id` 或 `source_path`）在新设备无旧 SQLite 下可恢复
- [ ] M6: Settings window UI（targets/endpoints/schedule/recovery key）+ Popover Settings tab 最小改动（仅增加打开 window 的入口）
- [ ] M7: daemon 按 target schedule 触发（默认继承全局；override 生效）+ 多 endpoint 支持
- [ ] M8: tests + docs updates（覆盖 config/crypto/bootstrap；更新 README/architecture）

## 约束与风险（Constraints & Risks）

- Telegram Bot API 无法枚举历史文件：跨设备恢复必须依赖可发现的 bootstrap 指针（例如 pinned message）或用户手动提供指针。
- UI 约束：Popover 现有视觉与信息架构不得做“超出合理范围”的改动；Settings window 采用标准 macOS Preferences 风格（避免在内容区自制 tabs/pills），Popover 内只做最小必要变更（例如新增一个打开 Settings window 的入口）。
- endpoint/账号风险：
  - chat 失效（退群/踢出 bot/拉黑 bot/解散群/删号等）会导致无法继续上传；且若 pinned bootstrap/catalog 丢失，将阻断新设备恢复。
  - 若 endpoint 使用的是 **私聊（bot ↔ 用户）**：当该用户账号被删号/不可用时，该 chat 往往会变为不可访问（Bot API 层面可能表现为 `chat not found` 等），从而同时影响“上传”和“通过 pinned message 发现 bootstrap”的能力。
  - 若 endpoint 使用的是 **群组/超级群/频道**：单个成员删号通常不影响 chat，但“群被解散/频道被删/bot 被移除/置顶消息被取消置顶或被删除”仍会破坏 bootstrap 可发现性。
  - bot 创建者账号被删号：Telegram 未公开保证 bot 的生命周期与所有权迁移行为；保守起见应视为运维风险（例如无法通过 BotFather 管理/轮换 token）。实现层面：只要 token 仍有效，备份/恢复可继续工作；若 token 被撤销或 bot 被封禁，则该 endpoint 的远端数据将不可再读取（即便仍持有金钥）。
  - bot token 泄露意味着该 endpoint 的远端密文可被读取（仍需 master key 才能解密）。
- 多 endpoint 的 provider namespace 若设计不当，可能导致“用错 file_id”或 dedup 污染，最终造成 restore/verify 失败或错误恢复。

## 开放问题（需要主人决策）

None

## 假设（Assumptions，需要确认）

None

## UI 设计（Design）

- Popover 现有 UI 基准：`docs/design/ui/liquid-glass-popover-settings.png`（及同名 `.svg`）
- Settings window（Targets）：[design/settings-window-targets.svg](./design/settings-window-targets.svg)
- Settings window（Security / 金钥）：[design/settings-window-security.svg](./design/settings-window-security.svg)
- Settings window（Schedule）：[design/settings-window-schedule.svg](./design/settings-window-schedule.svg)
- Popover（Settings tab 最小改动）：[design/popover-settings-minimal.svg](./design/popover-settings-minimal.svg)
