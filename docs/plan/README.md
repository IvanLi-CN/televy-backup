# 计划（Plan）总览

本目录用于管理“先计划、后实现”的工作项：每个计划在这里冻结范围与验收标准，进入实现前先把口径对齐，避免边做边改导致失控。

## 快速新增一个计划

1. 分配一个新的 `ID`（推荐 5 位 nanoId 风格；兼容四位数字；查看下方 Index，确保未被占用）。
2. 新建目录：`docs/plan/<id>:<title>/`（`<title>` 必须为 kebab-case 的短 slug）。
3. 在该目录下创建 `PLAN.md`（模板见下方“PLAN.md 写法（简要）”）。
4. 在下方 Index 表新增一行，并把 `Status` 设为 `待设计` 或 `待实现`（取决于是否已冻结验收标准），并填入 `Last`（通常为当天）。

## 目录与命名规则

- 每个计划一个目录：`docs/plan/<id>:<title>/`
- `<id>`：推荐 5 位 nanoId 风格（`[23456789abcdefghjkmnpqrstuvwxyz]{5}`），并兼容四位数字（`0001`–`9999`）；一经分配不要变更。
- `<title>`：短标题 slug（必须 kebab-case，避免空格与特殊字符）；目录名一经创建不再变更。
- 人类可读标题写在 Index 的 `Title` 列；标题变更只改 `Title`，不改目录名。

## 状态（Status）说明

仅允许使用以下状态值：

- `待设计`：范围/约束/验收标准尚未冻结，仍在补齐信息与决策。
- `待实现`：计划已冻结，允许进入实现阶段（或进入 PM/DEV 交付流程）。
- `部分完成（x/y）`：实现进行中；`y` 为该计划里定义的里程碑数，`x` 为已完成里程碑数（见该计划 `PLAN.md` 的 Milestones）。
- `已完成`：该计划已完成（实现已落地或将随某个 PR 落地）；如需关联 PR 号，写在 Index 的 `Notes`（例如 `PR #123`）。
- `作废`：不再推进（取消/价值不足/外部条件变化）。
- `重新设计（#<id>）`：该计划被另一个计划取代；`#<id>` 指向新的计划编号。

## `Last` 字段约定（推进时间）

- `Last` 表示该计划**上一次“推进进度/口径”**的日期，用于快速发现长期未推进的计划。
- 仅在以下情况更新 `Last`（不要因为改措辞/排版就更新）：
  - `Status` 变化（例如 `待设计` → `待实现`，或 `部分完成（x/y）` → `已完成`）
  - `Notes` 中写入/更新 PR 号（例如 `PR #123`）
  - `PLAN.md` 的里程碑勾选变化
  - 范围/验收标准冻结或发生实质变更

## PLAN.md 写法（简要）

每个计划的 `PLAN.md` 至少应包含：

- 背景/问题陈述（为什么要做）
- 目标 / 非目标（做什么、不做什么）
- 范围（in/out）
- 需求列表（MUST）
- 验收标准（Given/When/Then + 边界/异常）
- 非功能性验收/质量门槛（测试策略、质量检查等按仓库已有约定）
- 文档更新（需要同步更新的项目设计文档/架构说明/README/ADR）
- 里程碑（Milestones，用于驱动 `部分完成（x/y）`）
- 风险与开放问题（需要决策的点）

## Index（固定表格）

| ID   | Title | Status | Plan | Last | Notes |
|-----:|-------|--------|------|------|-------|
| 0001 | TelevyBackup MVP（Telegram 存储 + 差异备份） | 已完成 | `0001:telegram-backup-mvp/PLAN.md` | 2026-01-20 | PR #4；实现分支：`feat/0001-telegram-backup-mvp` |
| 0002 | 小对象打包降低 Bot API 调用频率（Pack） | 已完成 | `0002:small-object-packing/PLAN.md` | 2026-01-20 | PR #7；已冻结：启用阈值（>10 或 >32MiB）+ pack 目标 32MiB + hard max 49MiB；实现分支：`feat/0002-small-object-packing` |
| 0003 | Sync 日志落盘与可排查性（每轮独立日志 + env 配置日志等级） | 已完成 | `0003:sync-logging-durability/PLAN.md` | 2026-01-20 | 已落地：每任务一份日志（NDJSON）+ 结束前 `flush+fsync` + `TELEVYBACKUP_LOG`→`RUST_LOG`→default(debug)；实现分支：`feat/0003-sync-logging-durability` |
| 0004 | Telegram 通信升级为 MTProto API（MTProto-only，移除 Bot API） | 已完成 | `0004:telegram-mtproto-storage/PLAN.md` | 2026-01-22 | 实现分支：`feat/0004-telegram-mtproto-storage`；已落地：MTProto-only + `tgmtproto:v1` + vault/secrets store + mtproto helper（独立构建）；旧 `telegram.botapi` snapshot 不再支持（需重新备份） |
| 0005 | 设置窗口独立化与多备份目录（多 Telegram Endpoint + 金钥恢复） | 已完成 | `0005:multi-backup-directories-keyed-restore/PLAN.md` | 2026-01-22 | 实现分支：`feat/0005-multi-backup-directories-keyed-restore`；已落地：targets/endpoints + pinned bootstrap/catalog + per-target schedule + TBK1 金钥；UI 对齐现有视觉 |
| 0006 | Chunking 分块上限调整（按存储模式 + 内存预算） | 已完成 | `0006:chunking-max-bytes/PLAN.md` | 2026-01-23 | 已落地：upload max 128MiB；pack target 64MiB±8MiB；max entries/pack=32；`PACK_ENABLE_MIN_OBJECTS=10` 不变；实现分支：`feat/0006-chunking-max-bytes` |
| 0007 | Pack 上传后台并发（scan 与 upload 解耦） | 已完成 | `0007:background-pack-uploads/PLAN.md` | 2026-01-23 | 已冻结：队列双阈值（jobs+bytes）+ 等待回压；`min_delay_ms` 全局节流；限速来源 `telegram.rate_limit.*`；覆盖所有 upload（pack/直传/index）；实现分支：`feat/0007-background-pack-uploads` |
| 0008 | 状态弹出界面移除日志页（日志仅落盘） | 已完成 | `0008:status-popup-file-logging/PLAN.md` | 2026-01-24 | 已落地：Popover 移除 Logs + 移除 UI 内存日志缓存；UI 日志 `ui.log` 与 per-run logs 同目录；Settings 提供 Open logs（单入口）。实现分支：`feat/0008-status-popup-file-logging` |
| 0009 | Settings：Endpoints 独立配置页（Targets 仅绑定） | 已完成 | `0009:endpoints-settings-page/PLAN.md` | 2026-01-24 |  |
| 0010 | 状态弹窗重做：全局网络 + 多目标面板 + 开发者视图 | 已完成 | `0010:status-popover-dashboard/PLAN.md` | 2026-01-26 | 实现分支：`feat/0010-status-popover-dashboard` |
| 0011 | daemon 状态 IPC：替换 file-based 状态源 | 已完成 | `0011:daemon-status-ipc/PLAN.md` | 2026-01-26 | 实现分支：`feat/0011-daemon-status-ipc` |
| 0012 | 备份远端索引权威 + 本地自动同步（remote-first） | 已完成 | `0012:remote-first-index-sync/PLAN.md` | 2026-01-29 | 实现分支：`feat/0012-remote-first-index-sync` |
| 0013 | MTProto dialogs picker（自动选可用 chat_id） | 已完成 | `0013:mtproto-dialogs-picker/PLAN.md` | 2026-01-29 | CLI+UI e2e 已确认；状态面板 Up/UpTotal 上传中实时更新 |
| kaa5e | Targets 主界面与执行记录（按目标聚合 backup/restore/verify） | 已完成 | `kaa5e:targets-runs-main-window/PLAN.md` | 2026-02-01 | 实现分支：`feat/kaa5e-targets-runs-main-window` |
| nvr79 | 开发期绕过 Keychain（codesign + vault key；daemon-only） | 已完成 | `nvr79:avoid-keychain-in-dev/PLAN.md` | 2026-01-28 | 已落地：`TELEVYBACKUP_DISABLE_KEYCHAIN` + `vault.key` + daemon control IPC + tests |
| kpmqp | 修复 daemon IPC 可靠性（解锁 Recovery Key/Verify） | 已完成 | `kpmqp:fix-daemon-ipc-sockets/PLAN.md` | 2026-01-31 | PR #30；实现分支：`fix/kpmqp-daemon-ipc-sockets`；待主人验收 |
| fn4ny | Settings：配置整包导出/导入（keyed config bundle） | 已完成 | `fn4ny:config-bundle-export-import/PLAN.md` | 2026-02-05 | 实现分支：`feat/fn4ny-config-bundle-export-import`；待主人验收 |
| fwwqp | CLI events 实时状态与 GUI 进度一致性修复（flush + progress） | 已完成 | `fwwqp:events-live-task-ui/PLAN.md` | 2026-02-05 | PR #34 |
| dxddw | Import bundle：Targets 增加更换目录按钮 | 已完成 | `dxddw:import-bundle-target-change-directory/PLAN.md` | 2026-02-06 | PR #35 |
| r6ceq | 索引按 Endpoint 隔离 + 禁止 chat 复用 | 待实现 | `r6ceq:endpoint-scoped-index-chat-uniqueness/PLAN.md` | 2026-02-01 |  |
| 4fexy | Master key 轮换（可暂停/继续/取消） | 待实现 | `4fexy:master-key-rotation/PLAN.md` | 2026-02-01 | 依赖：`r6ceq` |
