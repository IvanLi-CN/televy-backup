# TelevyBackup MVP（Telegram 存储 + 差异备份）（#0001）

## 状态

- Status: 已完成
- Created: 2026-01-18
- Last: 2026-01-19

## 已冻结决策（Decisions, frozen）

- Telegram 接入：Bot API（官方服务器）
- Telegram 存储位置：与 Bot 的私聊（`chat_id` 固定）
- 索引上传：分片上传（加密）
- 密钥管理：macOS Keychain

## 背景 / 问题陈述

- 需要一个运行在 macOS 的桌面应用，用 Telegram 作为远端存储，把 Time Machine 备份盘的数据做“差异同步 + 去重 + 加密”，以尽量小的增量传输成本完成周期性备份。
- 备份不仅要“上传成功”，还要可恢复并能校验完整性；同时需要可观察（进度/失败原因/统计）与可控（取消、重试、限速）。

## 用户与场景（Users & Scenarios）

**用户**

- 个人用户：单台 macOS 机器，使用外接盘或本地磁盘保存 Time Machine 备份，想把备份再同步到 Telegram 做“异地副本”。

**核心场景**

- 手动备份：用户选择源目录并点击“开始备份”，等待完成并查看统计与结果。
- 定时备份：按小时/按天自动触发备份，失败可重试并在 UI 中可追踪。
- 恢复：用户选择某个快照并恢复到目标目录，随后校验恢复结果。

## 目标 / 非目标

### Goals

- 提供一个可用的 MVP：能对指定源目录生成一次“备份快照（snapshot）”，并把新增/变化的数据上传到 Telegram；能从任意快照恢复并校验。
- 传输成本尽量低：使用分块（chunking）+ 内容寻址（hash）去重；尽量复用 Telegram 端已存在的对象（不重复上传）。
- 端到端安全：分块后加密上传；密钥不写入仓库、不明文落盘（按约定的密钥管理策略）。
- 可观测/可调度：提供任务进度、统计与错误展示；支持按小时/按天触发（调度落地：launchd / brew services 的关系与最终选型在本计划内冻结）。

### Non-goals

- 作为 Time Machine 官方支持的“目标盘”（Time Machine 的官方工作流不在本计划范围内）。
- 字节级增量（对同一文件做二进制差分、只上传变化字节）。
- 多用户/多设备共享同一仓库（本版本固定单用户单机）。

## 约束与风险（Constraints & Risks）

### 约束（Constraints）

- 平台：macOS（桌面端 Tauri）。
- 存储：Telegram Bot API（官方服务器）+ 与 Bot 私聊。
- 上传限制（Bot API）：通过 `multipart/form-data` 上传“其他文件”最大 50MB；通过 `file_id` 复用发送无限制；通过 HTTP URL 方式有更小限制（不作为主路径）。该限制约束 chunk 与索引 part 的体积（需要考虑压缩/加密开销）。
- 隐私：任何敏感凭据不得进入仓库；日志与索引不得泄露明文敏感数据。

### 风险（Risks）

- Telegram 风控/限速/账号风险导致备份不可用或数据不可取回。
- 索引增长导致元数据文件变大，影响上传/下载与恢复速度。
- 源目录发生频繁写入会导致“备份视图不一致”，影响可恢复性与校验口径。

## 范围（Scope）

### In scope

- 备份（Backup）：扫描源目录 → CDC 分块 → 加密 → 上传缺失块 → 写入/更新 SQLite 索引 → 上传索引（或等价元数据）。
- 恢复（Restore）：拉取索引 → 拉取缺失块 → 重组文件 → 校验。
- 校验（Verify）：对快照进行一致性/完整性检查（块存在性、哈希匹配、可重组）。
- 存储适配层（Storage Adapter）：面向 Telegram 的上传/下载/复用接口（MVP 先实现一个明确路径）。
- Tauri 桌面 UI：任务发起、进度、统计、错误、设置入口（MVP 重点是可用与可观察）。

### Out of scope

- 直接操作 Time Machine 的内部快照机制（不做 APFS snapshot / 一致性冻结）。
- 服务端/云端组件（除非采用“自建 Local Bot API Server”被明确选为实现路径）。

## 需求（Requirements）

### MUST

- 备份任务：可对一个本地目录生成快照，并仅上传缺失/变化的数据。
- 内容定义分块（CDC）：提供可配置的 `min/avg/max` chunk size。
- 内容寻址去重：chunk id 基于哈希（例如 `blake3`）；相同 chunk 不重复上传。
- 加密：每个 chunk 加密后上传；索引中记录足够信息以供恢复（但不泄露明文）。
- SQLite 索引：记录快照、文件元数据、chunk 序列、以及 Telegram 侧对象引用（例如 `file_id`/消息定位信息）。
- 恢复：给定快照 id，能恢复到目标目录，并对恢复结果进行校验。
- UI：能展示任务状态、进度、错误；能发起备份/恢复/校验。
- 秘密信息：Bot token 与主密钥只存于 Keychain；配置文件仅记录引用与非敏感参数。
- 断点续传：任务中断/重启后可继续（至少做到“已上传 chunk 不重复上传”）。
- 限速与并发：可配置并发数与节流（避免触发 Telegram 风控）。
- 保留策略：必须提供可配置的“保留最近 N 个快照”策略；删除快照不做远端 chunk GC（只影响本地可见性）。

### Non-goals（明确不做）

- 更强一致性：对源目录生成一致性视图（APFS snapshot / 时间点冻结）。

## 技术选型（Tech Stack, frozen）

- Desktop：Tauri v2（macOS）
- Frontend：Vite + React + TypeScript（Tauri WebView UI）
- Backend（Rust）：async（Tokio），并通过 Tauri commands/events 与前端交互
- Index：SQLite（本地索引与任务状态）
- Chunking：CDC（`min/avg/max` 可配；chunk 上限受 Bot API 50MB 约束）
- Hash：BLAKE3（内容寻址）
- Crypto：AEAD（XChaCha20-Poly1305；格式固定：`version(1) + nonce(24B) + ciphertext+tag`；AD 固定为 `chunk_hash`）
- Compression：Zstd（主要用于索引单文件压缩）
- HTTP：`reqwest`（与 Telegram Bot API 交互）
- Secrets：macOS Keychain（Bot token、主密钥）
- Scheduling：launchd（通过 `brew services` 以用户级 `LaunchAgent` 方式管理；App 不需要常驻）

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Tauri commands（invoke） | RPC | internal | New | ./contracts/rpc.md | app | web | 前端通过 `invoke()` 调后端 |
| Tauri events（progress） | Event | internal | New | ./contracts/events.md | app | web | 进度/状态推送 |
| SQLite schema（index） | DB | internal | New | ./contracts/db.md | core | app | 索引与任务状态 |
| Local config & cache layout | File format | internal | New | ./contracts/file-formats.md | core | app | 配置/缓存/密钥不落盘约定 |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/rpc.md](./contracts/rpc.md)
- [contracts/events.md](./contracts/events.md)
- [contracts/db.md](./contracts/db.md)
- [contracts/file-formats.md](./contracts/file-formats.md)

## 验收标准（Acceptance Criteria）

### Core path

- Given 选择了一个源目录与 Telegram 存储配置，When 发起备份，Then 生成一个新的 snapshot，并仅上传缺失 chunk，任务完成后可在 UI 看到统计（上传字节、去重率、耗时）。
- Given 备份任务成功完成，When 查看该 snapshot 详情，Then 能看到该 snapshot 对应的“远端索引单文件”引用已写入（可用于恢复路径的 `download index → restore`）。
- Given 同一源目录未发生变化，When 再次发起备份，Then 上传字节≈0（允许有少量元数据写入），并生成一个可追溯的 snapshot 记录。
- Given 已存在一个 snapshot，When 发起恢复到空目录，Then 输出文件树与校验结果为通过（hash/大小/文件数一致）。

### 边界/异常

- Given 网络波动或 Telegram 限速，When 备份进行中发生失败，Then 任务可重试并在 UI 展示“可重试/不可重试”的错误原因。
- Given 任务被用户取消，When 点击取消，Then 任务进入 `Cancelled` 状态并释放资源；再次运行不会重复上传已存在 chunk。
- Given 索引缺失或损坏，When 进行恢复/校验，Then 明确报错并提供可操作的修复动作（例如“重新下载索引”或“从另一个 snapshot 恢复”）。
- Given 缺失任意一个 index part，When 执行 verify，Then 报错包含可定位信息（至少包含 `snapshot_id` 与缺失的 `part_no`），并标记为不可自动修复。
- Given 任意一个 chunk 缺失，When 执行 verify，Then 报错包含缺失的 `chunk_hash`，并标记为可重试（网络/限速）或不可重试（远端确实缺失）两类之一。
- Given chunk hash 不一致（密钥不匹配或数据损坏），When 执行 verify，Then 报错并标记为不可重试。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: CDC 切分稳定性、hash 计算、加密/解密 round-trip、SQLite 基础 CRUD 与约束。
- Integration tests: 使用“本地假存储（in-memory / fs mock）”模拟上传/下载，跑一次端到端备份→恢复→校验。
- E2E tests (required): 必须覆盖 UI 基本流程（设置 → 发起任务 → 展示进度 → 展示结果/错误）。
  - Desktop E2E 的首选方案为 WebDriver（`tauri-driver`）。但按 Tauri 官方文档：桌面端 WebDriver 目前仅支持 Windows/Linux，macOS 桌面因缺少 WKWebView driver 暂不支持。
  - 本项目固定采用 WebdriverIO + `tauri-driver` 作为 E2E 测试栈；E2E 固定在 CI（Linux runner）上执行；macOS 本机必须执行手工冒烟清单（不引入额外 UI 自动化）。

### macOS 手工冒烟清单（required）

- 安装并启动 App（首次启动无崩溃）
- Settings：写入 bot token（Keychain）与 chat_id，点击 Validate，结果为成功
- Backup：选择一个小目录（含多个文件），启动备份，能看到 progress/state，最终 succeeded
- Restore：选择该 snapshot，恢复到空目录，最终 succeeded 且文件数量/大小一致
- Verify：对该 snapshot 执行 verify，最终 succeeded

### Quality checks

- Rust: `cargo fmt` / `cargo clippy -D warnings` / `cargo test`
- Web: `biome check` / `vite build`

## 文档更新（Docs to Update）

- `docs/requirements.md`: 与本计划冻结后的范围/术语/验收标准对齐（避免重复与冲突）。
- `README.md`: 补充“运行方式、配置入口、数据目录与风险提示（Telegram 存储风险）”。
- `docs/architecture.md`: 必须新增并记录核心数据流与威胁模型。

## 里程碑（Milestones）

- [x] M1: 冻结 Telegram 存储路径与鉴权方案（Bot API + 私聊 Bot）并在契约中固化
- [x] M2: 冻结 SQLite schema 与索引上传策略（索引加密分片上传 + manifest）
- [x] M3: 备份管线 MVP（scan → chunk → encrypt → upload → index）
- [x] M4: 恢复/校验 MVP（fetch index → fetch chunks → reassemble → verify）
- [x] M5: UI MVP（任务列表、进度、错误、统计、基础设置）
- [x] M6: 调度与保留策略（小时/天触发；GC/保留）
- [x] M7: 打包与发布（brew 安装 + `brew services` 管理 + 升级不丢数据）

## 方案概述（Approach, high-level）

- 分层：`core`（chunk/encrypt/index）与 `app`（Tauri commands + UI）解耦；存储通过 `Storage Adapter` 抽象以降低切换成本。
- 最小传输：chunk 级去重优先；如果 Telegram 支持通过 `file_id` 复用对象，则实现“零上传复用”路径。
- 一致性口径：以“扫描时刻的一致性”为目标（不做 APFS snapshot）。

## 调度设计（Scheduling, draft）

目标：支持 hourly/daily 触发，并且在 brew 安装后可被用户可靠启用/停用。

在 macOS 上：

- `launchd` 是系统的任务/服务管理器；
- `brew services` 是对 `launchctl` 的封装：不加 `sudo` 时通常以用户级 `LaunchAgents`（登录后运行）方式管理；加 `sudo` 时通常以系统级 `LaunchDaemons`（开机运行）方式管理。

已确认（owner decision）：

- 采用 `brew services` 常驻后台（方案 A）
- 用户级（`~/Library/LaunchAgents`，无需 `sudo`）

落地方式（A, chosen）：

- A) `brew services start televybackupd`：常驻 daemon 进程，进程内用定时器触发备份（优点：统一管理/状态可查询；缺点：需要常驻）。

## 安装与发布（Homebrew, required）

目标：提供一个最小可用的“安装 + 服务管理 + 升级不破坏数据”的闭环，与本计划的调度选型一致（用户级 `brew services` / LaunchAgent）。

### MUST

- 提供 brew 安装方式：formula（后台 daemon）+ cask（GUI app）。
- `brew services start` 能启动后台（用户级），并能按 schedule 触发一次备份（以本地 tasks/snapshots 可追溯为准）。
- 升级后不丢数据：SQLite / config 保持可读；Keychain secrets 不需要重新输入（除非用户主动擦除）。
- 日志路径与数据目录路径固定且可在文档中明确（不得泄露 secrets）。

## 风险与开放问题（Risks & Open Questions）

### 开放问题（需要主人决策）

None（本计划的关键决策已冻结，可进入实现阶段）。

## 假设（Assumptions，需要确认）

- 使用单用户单机；不做多设备并发写入同一仓库。
- 源目录可读，且备份期间不会遭遇频繁并发写入导致无法接受的不一致。

## 参考（References）

- Telegram Bot API “Sending files” / `file_id` 复用与上传限制（用于 chunk/索引体积约束）：https://core.telegram.org/bots/api#sending-files
- Tauri v2 commands / capabilities（用于 RPC/Event 安全模型）
- Tauri WebDriver tests / `tauri-driver`（用于 E2E 方案与平台限制）：https://tauri.app/develop/tests/webdriver/
- Homebrew `brew services`（用于 `launchd` / `LaunchAgents` 的调度与常驻管理）：https://docs.brew.sh/Manpage#services-subcommand
