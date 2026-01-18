# Requirements: TelevyBackup

## 1) Problem statement

需要在 macOS 上对 Time Machine 备份盘进行周期性差异备份，并将数据可靠上传到 Telegram 作为存储目标，尽量降低重复传输成本，同时可恢复并验证备份完整性。

## 2) Goals

- [ ] 将 Time Machine 备份盘内容按固定/可配置分块并加密后上传到 Telegram 存储。
- [ ] 支持按小时或按天触发差异同步，避免重复上传未变化数据。
- [ ] 使用 SQLite 保存索引/清单并在每次备份完成后同步到存储目标。
- [ ] 可从存储目标恢复出原始文件结构并进行完整性校验。

## 3) Non-goals / out of scope

- 直接替代 Time Machine 的官方目标盘功能。
- 块内字节级增量（仅上传文件内变更字节）。
- GUI 前端（优先 CLI/服务化）。

## 4) Users & scenarios

**Users**
- 使用 Time Machine 的个人用户/开发者。

**Top scenarios**
- 每小时/每天自动对外接 Time Machine 盘进行差异备份并上传。
- 需要从 Telegram 存储快速恢复某个历史状态。

## 5) Functional requirements (prioritized)

### MUST
- 支持配置源目录（Time Machine 备份盘挂载点）。
- 将文件拆分为 1–10MB（可配置）大小的块并加密存储。
- 使用 SQLite 记录：快照版本、文件元数据、块序列、块哈希与存储对象 ID。
- 仅上传缺失/变化的块（基于哈希索引去重）。
- 断点续传与失败重试（可配置重试策略）。
- 每次备份完成后将 SQLite 索引文件上传到存储目标。
- 提供恢复流程：拉取索引与块并重组出文件。

### SHOULD
- 支持内容定义分块（CDC）以降低变更扩散。
- 支持并发上传与限速配置。
- 提供完整性校验（文件级与块级）。

### COULD
- 保留多版本快照与按策略清理旧快照。
- 支持只恢复某个子目录或单文件。

## 6) Non-functional requirements

- **Performance:** 在本地磁盘与网络带宽允许范围内，尽量减少重复上传；支持并发与限速。
- **Reliability:** 上传失败可重试；任务中断可恢复；索引与数据一致性可验证。
- **Security:** 分块后加密；密钥仅本地保存；上传链路使用安全通道。
- **Privacy/compliance:** 不上传明文敏感数据；索引中避免泄露敏感内容。
- **Accessibility:** CLI 输出清晰可读。
- **Observability:** 关键操作日志、失败原因、传输统计、校验结果。

## 7) Constraints

- **Platforms:** macOS（运行在有 Time Machine 备份盘的主机）。
- **Compatibility:** 与 Time Machine 备份目录结构兼容，不破坏原备份。
- **Dependencies/integrations:** Telegram 存储目标（通过选定的上传方式接入）。
- **Deadline/budget:** 未设定。
- **Policy:** 不依赖专有云存储服务。

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

- **Wire notes:** CLI 命令：backup / restore / verify / stats
- **Copy:** 清晰显示已上传块数、去重率、耗时与失败重试次数。
- **Edge states:** 无变更、网络中断、索引损坏、块缺失。

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

- 选择哪种 Telegram 接入方式（用户会话/MTProto 或其他）？
- 是否需要内容定义分块（CDC）还是固定分块即可？
- 加密算法与密钥管理策略是什么？
- 备份快照的保留与清理策略如何定义？
- 是否需要对 Time Machine 备份盘进行一致性快照？

## 12) Assumptions (needs confirmation)

- 仅单用户使用；不需要多用户共享或权限隔离。
- 本地磁盘可读取 Time Machine 备份盘且不会被同时写入导致不一致。
- Telegram 存储目标可长期稳定访问。
