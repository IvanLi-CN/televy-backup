# 端点索引二级拆分：Endpoint DB（一级）+ Snapshot Filemap DB（二级）+ 严格远端门禁（Fail Fast）（#t764g）

## 状态

- Status: 已完成
- Created: 2026-03-02
- Last: 2026-03-02

## 背景 / 问题陈述

- 现状：同一个 Telegram endpoint 下存在多个大目录（多个 targets / source_path）时，备份会反复上传/下载“包含其它目录历史文件映射”的巨型 index DB（SQLite），导致：
  - `index_sync`/index upload 时间远大于 data upload，表现为“带宽没吃满 / 卡在 index”
  - 多机/重装时远端同步成本过高
- 根因：当前 endpoint index DB 同时承担了两类职责：
  - 全局元数据（`snapshots/remote_indexes/...`）
  - 每个 snapshot 的完整文件映射（`files/file_chunks`），且随快照线性增长
- 一致性问题：当前 daemon/cli 对远端不可用采取 best-effort（skip index_sync / bootstrap 更新失败仍算成功），会导致“上传了数据但无法从远端发现/同步”的半成品状态。

## 目标 / 非目标

### Goals

- 两级索引（Two-level index）：
  - **Endpoint DB（一级）**只保留全局去重/元数据 + 远端 filemap 指针（小且稳定）
  - **Snapshot Filemap DB（二级）**每个 snapshot 一份，仅包含该 snapshot 的 `files/file_chunks`（大但只与当前 target 的文件规模相关）
- 远端严格门禁（默认 fail fast）：
  - bootstrap 读取失败 / endpoint DB 下载失败 / 关键 publish（endpoint DB 或 bootstrap update）失败 => 备份直接失败
  - 避免“备份跑完但远端不可恢复”的不一致状态
- 兼容旧格式：
  - 旧 snapshot index（单库且包含 `chunk_objects`）仍可 restore/verify
  - 新格式 snapshot filemap DB **不含** `chunk_objects` 时，restore/verify 自动下载 endpoint DB 并 `ATTACH` 联合查询

### Non-goals

- 引入 multi-sender / 多 helper 进程上传（涉及 session 共享一致性）
- 物理删除 Telegram 历史消息（仅做“从 bootstrap/endpoint DB 不可达”的逻辑删除）

## 架构（Remote + Local）

### Remote：Pinned Bootstrap Catalog（仍为 v1 JSON，可扩字段）

- 继续保留（语义调整）：`targets[].latest { snapshot_id, manifest_object_id }`
  - `manifest_object_id` **指向二级 Snapshot Filemap DB 的 manifest**
- 新增可选字段（向后兼容）：
  - `endpointLatest: { endpointIndexId, manifestObjectId }`
    - `endpointIndexId = "televy.endpoint_index.v1:" + <chat_id>`
    - `manifestObjectId` 指向一级 Endpoint DB 的 manifest

### Remote：Index Objects

- 一级 Endpoint DB（远端对象）
  - 包含：`snapshots / remote_indexes / remote_index_parts / tasks (+ endpoint_state)`
  - 说明：全端点去重映射（`chunks/chunk_objects`）在 #3z7rj 中迁移为独立的 remote dedupe（Base + Delta + Catalog），不再由 endpoint DB 承载
  - 不再包含：任何 `files / file_chunks`
  - 由 bootstrap 的 `endpointLatest` 指向
- 二级 Snapshot Filemap DB（远端对象）
  - 每个 snapshot 一个 DB，仅包含：
    - `snapshots(单行) / files / file_chunks / chunks(满足 FK)`
  - 不包含：`chunk_objects`
  - 其 manifest object id 记录在一级 endpoint DB 的 `remote_indexes(snapshot_id=...)`

### Local：文件布局

- 一级 endpoint DB（沿用现有路径）
  - `~/Library/Application Support/TelevyBackup/index/index.<endpoint_id>.sqlite`
- 二级 filemap cache（新增）
  - `~/Library/Application Support/TelevyBackup/index/filemaps/<endpoint_id>/<snapshot_id>.sqlite`
  - best-effort 清理：随着 retention 删除本地缓存（远端不做物理删除）

## 行为规格（Behavior Spec）

### Backup（strict + two-level）

1) Preflight（必须成功，否则 fail fast）

- 读取 pinned bootstrap catalog：
  - catalog 缺失：允许“首次初始化”继续（但后续 publish 仍必须成功）
  - Telegram transient error：fail fast
- 若 bootstrap 存在 `endpointLatest`：
  - 若本地 endpoint DB 未同步到该 manifest：下载 endpoint DB 并写入本地
- 计算 base_snapshot_id（从 endpoint DB 取该 source_path 的 latest snapshot）
  - 若存在 base_snapshot_id：解析 endpoint DB 的 `remote_indexes` 获取 base snapshot 的 filemap manifest，并下载二级 DB 到本地 cache

2) Scan + upload pipeline

- endpoint DB：只写元数据表（`snapshots/remote_indexes/remote_index_parts/tasks/endpoint_state`）
- dedupe DB：在 #3z7rj 中承担全端点去重映射（`chunks/chunk_objects`）
- filemap DB（每个 snapshot 新建本地 sqlite）：写 `snapshots/files/file_chunks/chunks`
- base-chunk-copy：
  - 从 base filemap DB 查 `files/file_chunks` 并复制到新 filemap DB（不依赖 endpoint DB 的 files/file_chunks）

3) Index publish（必须成功，否则备份失败）

- 上传二级 filemap DB：
  - 生成 IndexManifest v1（`snapshot_id = <snapshot_id>`），分片加密上传
  - 将 manifest/parts 写入 endpoint DB 的 `remote_indexes/remote_index_parts`（key=snapshot_id）
- 导出并上传一级 endpoint DB（不含 files/file_chunks）：
  - 生成 IndexManifest v1（`snapshot_id = <endpointIndexId>`），分片加密上传
- 更新 pinned bootstrap catalog（一次保存）：
  - `endpointLatest = { endpointIndexId, manifestObjectId }`
  - `targets[].latest = { snapshot_id, manifest_object_id = <filemap manifest> }`

### Restore / Verify（two-level aware）

- 拉取 bootstrap：
  - 得到 endpointLatest 与 target.latest(filemap)
- 下载 endpoint DB（若 endpointLatest 存在）
- 下载 filemap DB（目标 snapshot）
- 打开 filemap DB 并 `ATTACH` endpoint DB：
  - file 列表来自 filemap DB（`files/file_chunks`）
  - object_id 映射来自 dedupe DB（#3z7rj）或旧格式 endpoint DB（`chunk_objects`）
- 兼容旧 snapshot index：
  - 若下载到的 DB 自带 `chunk_objects` 且可满足查询：允许不下载 endpoint DB 直接 restore/verify

## Schema / Migrations

- 新增 migration：`crates/core/migrations/0004_endpoint_state.sql`
  - `endpoint_state(key TEXT PRIMARY KEY, value TEXT NOT NULL)`
  - 用于记录本地“已同步/已发布”的 endpointLatest manifest id 等状态，避免重复下载

## 验收标准（Acceptance Criteria）

- 两级索引生效：
  - 同一 endpoint 下有 2+ 个大 target 时，备份 target A 上传的 index bytes ≈ `A 的 filemap DB + endpoint DB`，不随 target B 文件规模线性增长
- 严格门禁：
  - bootstrap/endpoint/filemap 任一关键步骤失败 => 备份在 prepare/index_sync 阶段直接失败（不会继续扫盘跑数小时）
  - backup 成功但 bootstrap 更新失败 => 整体 run 标记失败
- restore latest：
  - 新格式（filemap 无 chunk_objects）可完成 restore/verify（通过 endpoint DB + ATTACH）
  - 旧格式（单库）仍兼容

## 质量门槛（Quality Gates）

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-features`

## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: Spec 与 docs/specs/README.md 索引落地
- [x] M2: bootstrap v1 扩展（endpointLatest）+ 单测
- [x] M3: endpoint DB upload/download（export 不含 files/file_chunks）+ endpoint_state
- [x] M4: snapshot filemap DB 生成/上传（remote_indexes 指向二级 DB）
- [x] M5: backup pipeline 改造（scan 写二级 DB；base-chunk-copy 读 base filemap DB）
- [x] M6: restore/verify 改造（ATTACH 两级 DB；旧格式兼容）
- [x] M7: strict 门禁：去掉 best-effort continue；bootstrap update 失败 => run failed；全量测试回归
