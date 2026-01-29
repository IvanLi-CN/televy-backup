# Requirements: TelevyBackup

## 1) Problem statement

需要在 macOS 上对 Time Machine 备份盘进行周期性差异备份，并将数据可靠上传到 Telegram 作为存储目标，尽量降低重复传输成本，同时可恢复并验证备份完整性。

## 2) Goals

- [ ] 将 Time Machine 备份盘内容按固定/可配置分块并加密后上传到 Telegram 存储。
- [ ] 支持按小时或按天触发差异同步，避免重复上传未变化数据。
- [ ] 使用 SQLite 保存索引/清单并在每次备份完成后同步到存储目标。
- [ ] 可从存储目标恢复出原始文件结构并进行完整性校验。
- [ ] 提供 macOS 状态栏应用：用于查看状态、手动触发备份、查看日志与基础设置。

## 3) Non-goals / out of scope

- 直接替代 Time Machine 的官方目标盘功能。
- 块内字节级增量（仅上传文件内变更字节）。
- 复杂多窗口 GUI（仅做状态栏 popover + 必要的设置页）。

## 4) Users & scenarios

**Users**
- 使用 Time Machine 的个人用户/开发者。

**Top scenarios**
- 每小时/每天自动对外接 Time Machine 盘进行差异备份并上传。
- 需要从 Telegram 存储快速恢复某个历史状态。

## 5) Functional requirements (prioritized)

### MUST
- 支持配置源目录（Time Machine 备份盘挂载点）。
- 将文件使用 **CDC（内容定义分块）** 拆分为块并加密存储（默认 FastCDC；默认 `min=1MiB, avg=4MiB, max=10MiB`；均可配置）。
- 使用 SQLite 记录：快照版本、文件元数据、块序列、块哈希与存储对象 ID。
- 仅上传缺失/变化的块（基于哈希索引去重）。
- 断点续传与失败重试（可配置重试策略）。
- Telegram 存储接入：**MTProto-only**（配置 `telegram.mode = "mtproto"`）。
  - Telegram Bot API 已移除；历史 `telegram.botapi` snapshot/provider 不受支持，需要重新备份。
- 存储映射：chunk 上传对象为 `tgfile` 或 `tgpack`（pack 聚合多个加密 chunk blob；启用条件：待上传对象数 `> 10` 或总字节数 `> 32MiB`；pack 目标 32MiB、hard max 49MiB）。
- 每次备份完成后将 SQLite 索引文件上传到存储目标。
- 在 GUI 未打开的情况下，仍能按小时/每天自动触发备份任务（用户级 `launchd`）。
- 提供恢复流程：拉取索引与块并重组出文件。

## 6) Non-functional requirements

- **Performance:** 在本地磁盘与网络带宽允许范围内，尽量减少重复上传；支持并发与限速。
- **Reliability:** 上传失败可重试；任务中断可恢复；索引与数据一致性可验证。
- **Security:** 分块后加密；密钥仅本地保存；上传链路使用安全通道。
  - 生产默认口径：vault key 仅存于 macOS Keychain，用于解密本地加密 secrets store（`secrets.enc`）。
  - 开发期可选口径：允许通过环境变量禁用 Keychain 并改用 `vault.key` 文件（**安全性降级**，仅用于本地开发/调试；不应作为生产默认）。
- **Privacy/compliance:** 不上传明文敏感数据；索引中避免泄露敏感内容。
- **Accessibility:** CLI 输出清晰可读。
- **Observability:** 每轮 `backup|restore|verify` 生成一份独立的 NDJSON 日志文件（任务结束前 `flush+fsync`），可通过 `TELEVYBACKUP_LOG`/`TELEVYBACKUP_LOG_DIR` 配置；不混入 stdout/stderr 的机器可解析输出。

## 6.1) Crypto decision (frozen)

- AEAD: **XChaCha20-Poly1305**
- Rationale:
  - 与 AES-GCM 相比，XChaCha20-Poly1305 的 **nonce 更大（192-bit）**，在工程上更“抗误用”（更不容易因为 nonce 重复导致灾难性后果）。
  - 性能：在缺少 AES 硬件加速的平台上，ChaCha 系列通常更快；在有 AES 硬件加速的平台（含 Apple Silicon）上，AES-GCM 可能更快。但对“按 1–10MB 分块加密上传”的场景，瓶颈通常更容易落在 IO/网络/Telegram 速率限制上，而不是 AEAD 本身。
- Encoding:
  - 每块生成唯一随机 nonce，并将 `nonce + ciphertext + tag` 作为上传内容。
  - 去重使用“块内容哈希”而非密文（否则每次加密 nonce 不同会破坏去重）；nonce/加密元数据写入 SQLite。

## 7) Constraints

- **Platforms:** macOS 15+（Sequoia；运行在有 Time Machine 备份盘的主机）。
- **Compatibility:** 与 Time Machine 备份目录结构兼容，不破坏原备份。
- **Dependencies/integrations:** Telegram 存储目标（通过选定的上传方式接入）。
- **Deadline/budget:** 未设定。
- **Policy:** 不依赖专有云存储服务。

## 7.1) Proposed architecture & tech stack

本阶段采用 **原生 macOS 应用（SwiftUI/AppKit）** + **Rust（async）后端与常驻 daemon** 的组合：

- **GUI（native macOS）**
  - SwiftUI/AppKit UI（状态栏 popover + 设置页）。
  - UI 通过本地 `televybackup` CLI 发起任务，并读取 stdout 的进度/结果；避免把 secrets 通过 argv 传递。
- **Core（Rust library）**
  - `scan → chunk → encrypt → upload → index` 备份管线
  - `fetch index → fetch chunks → reassemble → verify` 恢复/校验管线
- **Daemon（Rust, scheduled runner）**
  - 由 `brew services`（用户级 LaunchAgent）常驻启动。
  - 进程内定时触发（hourly/daily），并在备份成功后执行快照保留策略（仅本地索引裁剪，不做远端 GC）。
- **Data**
  - SQLite：本地索引数据库（索引本身也会加密分片上传以支持恢复）。
  - Keychain：仅保存 vault key（用于解密本地加密 secrets store）；配置文件不落 secret 明文。
    - 生产默认：使用 Keychain（推荐）。
    - 开发期可选：`TELEVYBACKUP_DISABLE_KEYCHAIN=1` 时由 daemon 以 `vault.key` 文件承载 vault key（安全性降级）。
  - secrets store（`secrets.enc`）：保存 bot token / master key / MTProto 凭据与 session（加密落盘）。
- **Packaging**
  - Homebrew：formula（daemon）+ cask（GUI app）。
  - 升级不丢数据：配置与 SQLite 目录固定，secrets store 不需要重新输入（除非用户主动清除）。

## 8) Data & interfaces (if applicable)

- **Data model:**
  - Snapshot(id, created_at, source_path)
  - File(path, size, mtime, snapshot_id)
  - Chunk(hash, size, object_id, encryption_meta)
  - FileChunk(file_id, chunk_hash, seq)
- **API contract:**
  - backup run: scan -> chunk -> upload missing -> write index -> upload index
  - restore: fetch index -> fetch chunks -> reassemble -> verify
- **Migration/rollout:** 无数据库迁移要求；可通过新快照逐步启用。

## 9) UX / UI (if applicable)

- **UI form factor:** 原生 macOS 状态栏窗口（MVP 以“可用+可观察”为主）。
- **Views:** Settings / Backup / Restore / Verify / Logs（以及最小 Tasks 视图）。
- **Copy:** 清晰显示状态/阶段、上传统计与失败原因；错误提供可操作引导。
- **Edge states:** 无变更、网络中断、索引损坏、块缺失、vault key / secrets store 未配置。

## 10) Acceptance criteria

### Core path
- [ ] Given 已配置源目录与存储目标，when 触发备份，then 仅上传缺失块并生成新的快照索引。
- [ ] Given 无文件变化，when 再次备份，then 不上传任何数据且快照可标记为“无变更”。
- [ ] Given 备份中断，when 重新执行，then 能从断点继续而不重复上传已完成块。

### Edge cases
- [ ] 索引文件损坏时，系统能检测并给出可操作的错误提示。
- [ ] 存储目标缺失部分块时，恢复流程能报告缺失并停止或继续策略可配置。

### Observability
- [ ] 输出传输统计：总块数、上传块数、去重率、耗时与失败重试次数。

## 11) Open questions

None (v1 frozen)

## 12) Assumptions (needs confirmation)

- 仅单用户使用；不需要多用户共享或权限隔离。
- 本地磁盘可读取 Time Machine 备份盘且不会被同时写入导致不一致。
- Telegram 存储目标可长期稳定访问。
- AEAD 选择：**XChaCha20-Poly1305**（每块唯一 nonce；密钥材料存 secrets store，vault key 存 Keychain）。
- 快照保留策略：v1 **只追加不清理**。
- 分发：可接受无 Apple 开发者账号的侧载体验（需要用户手动放行）。
- CDC：v1 使用 FastCDC，默认参数 `min=1MiB, avg=4MiB, max=10MiB`。
- 调度：v1 使用 `launchd` 的 `StartInterval`。
