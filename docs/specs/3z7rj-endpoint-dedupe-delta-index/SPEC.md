# Endpoint 去重索引增量化：Remote Delta + 本地物化库 + 周期性 Compaction（#3z7rj）

## 状态

- Status: 已完成
- Created: 2026-03-02
- Last: 2026-03-02

## 背景 / 问题陈述

- 现状：即便已落地两级索引（endpoint meta DB + per-snapshot filemap DB），备份仍经常出现“index 上传占主要时间 / 小目录被大目录连累”的情况。
- 根因：endpoint meta DB 仍包含全端点去重映射（`chunks` + `chunk_objects`）。当一个 endpoint 下存在多个 targets 时：
  - Projects 等大目录会持续扩大去重表
  - Sync 等小目录每次备份仍需要上传整个 endpoint meta DB（包含全端点去重），形成跨目录耦合的固定税

## 目标 / 非目标

### Goals

- 本地仍保留“完整去重物化库”（dedupe materialized DB），以便：
  - 去重命中查询保持 O(1) 本地读取
  - 备份 scan 阶段可充分复用历史 chunk 映射（避免重传）
- 远端去重索引发布改为增量：
  - Base dedupe DB：偶尔/首次上传
  - Delta dedupe DB：每次备份仅上传新增/更新的去重条目
  - Dedupe Catalog：小 JSON，指向 base + deltas
  - Compaction：当 deltas 达到阈值（默认 128）时，重发 base 并清空 deltas
- 严格门禁（fail fast）：
  - 若 bootstrap 指向远端 dedupe catalog，则 catalog/base/delta 任一关键下载失败 => 备份失败
  - 若备份数据上传成功但 dedupe publish 或 bootstrap 更新失败 => run 失败（避免半成品）

### Non-goals

- 改造 `IndexManifest` 协议版本（仍使用 v1）
- 远端物理删除 Telegram 消息（仅通过索引不可达实现“逻辑删除”）
- 引入跨进程/多 session 并发上传优化（另开 spec）

## 架构（Remote + Local）

### Remote（Telegram）

1) Pinned Bootstrap Catalog（v1，向后兼容扩字段）

- 保留：`endpointLatest`（endpoint meta DB 指针）
- 新增：`endpointDedupeLatest: { endpointDedupeId, catalogObjectId }`（dedupe catalog 指针）

2) Dedupe Catalog object（加密 JSON）

- 加密 AAD（稳定且 scoped）：`"televy.endpoint_dedupe.catalog.v1:" + <scope>`
- 内容：
  - `base: { baseId, manifestObjectId }`
  - `deltas: [{ deltaId, manifestObjectId, createdAt, bytes? }, ...]`

3) Base / Delta dedupe DB objects（SQLite + zstd + 分片加密上传，复用 IndexManifest v1）

- Base ID（manifest snapshot_id / AAD）：
  - `"televy.endpoint_dedupe.base.v1:" + <scope>`
- Delta ID（manifest snapshot_id / AAD）：
  - `"televy.endpoint_dedupe.delta.v1:" + <scope> + ":" + <uuid_v4_simple>`

> `<scope>` 来自 `Storage::object_id_scope()`（MTProto 为 chat_id/peer）；无 scope（tests）回退 `Storage::provider()`。

### Local（磁盘）

- Endpoint meta DB（沿用两级索引的一级 DB）：
  - `~/Library/Application Support/TelevyBackup/index/index.<endpoint_id>.sqlite`
- Snapshot filemaps（沿用两级索引的二级 DB cache）：
  - `~/Library/Application Support/TelevyBackup/index/filemaps/<endpoint_id>/<snapshot_id>.sqlite`
- 新增：Dedupe materialized DB（本地完整去重缓存）：
  - `~/Library/Application Support/TelevyBackup/index/dedupe/dedupe.<endpoint_id>.sqlite`
- 新增：Pending dedupe spool DB（跨失败保留，确保最终一致性）：
  - `~/Library/Application Support/TelevyBackup/index/dedupe/pending.<endpoint_id>.sqlite`

## 行为规格（Behavior Spec）

### Preflight（CLI + daemon）

- 若 bootstrap 含 `endpointDedupeLatest`：
  - 若本地 dedupe DB 的 `endpoint_state["dedupe_catalog_object_id"]` 等于 `catalogObjectId`：跳过
  - 否则：下载 dedupe catalog -> 下载 base -> 逐个下载 delta 并 apply -> 原子写入本地 dedupe DB，并设置 state
- 若 bootstrap 不含 `endpointDedupeLatest`：不强制 dedupe sync；backup 可能进入 Enable 初始化模式

### Backup（core）

- 当启用 remote dedupe（Enable/Incremental）：
  - 去重查询从 dedupe DB 读取 `chunk_objects`
  - 新 chunk 的 `chunks` 写入 dedupe DB（不再写入 endpoint meta DB）
  - 上传成功后把 `chunk_objects` 写入：
    - dedupe materialized DB（用于后续 dedupe）
    - pending spool DB（用于远端 publish；只有 publish 完整成功才清空）
- Index publish（严格）：
  - snapshot filemap DB：保持现状（每 snapshot 上传一次）
  - endpoint meta DB：导出时排除 `chunks/chunk_objects`，仅上传小 meta
  - dedupe publish：
    - Enable：上传 base + 新 catalog，并写入 bootstrap.endpointDedupeLatest
    - Incremental：若 spool 非空，上传 delta 并更新 catalog；若 deltas>=阈值，改为 compaction（重发 base + 清空 deltas）

### Restore / Verify

- 若 bootstrap 提供 `endpointDedupeLatest`：优先下载/物化 dedupe DB，并 `ATTACH` 为 `dd` 用于 `chunk_objects` lookup
- 否则保持旧逻辑（endpoint DB `ep` 或单库 `chunk_objects` 兼容）

## 验收标准（Acceptance Criteria）

- 跨 target 去耦合：
  - 在一个 endpoint 下存在 Projects（大）与 Sync（小）两个 targets 时，Sync 的 index 上传量不再随 Projects 去重表增长而线性增长。
- 可重建性：
  - 在新机器上仅依赖 bootstrap + remote objects，可物化 dedupe DB 并完成 restore/verify（新格式 snapshot filemap DB 不含 `chunk_objects`）。
- 严格门禁：
  - 若 bootstrap 指向远端 dedupe catalog，而 catalog/base/delta 任一关键步骤失败，则备份 fail fast（不继续长时间 scan）。

## 质量门槛（Quality Gates）

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features`

## Change log

- 2026-03-02: 实现 Remote Dedupe（Base + Delta + Catalog）发布/物化、本地 pending spool，以及 restore/verify 优先使用 dedupe DB（并保持旧格式兼容）。
