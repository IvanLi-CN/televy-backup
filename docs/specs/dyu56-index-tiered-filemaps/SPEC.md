# 索引分级：Remote Index 仅保留每个 Source 最新文件映射（#dyu56）

## 状态

- Status: 部分完成（2/3）
- Created: 2026-03-02
- Last: 2026-03-02

## 背景 / 问题陈述

- 当前 endpoint 的本地索引库（`index/index.ep_<endpoint_id>.sqlite`）在 retention=7 且百万级文件规模时会膨胀到多 GiB：
  - `files` / `file_chunks` 会为**每个快照**存一份完整映射（尽管大多数文件未变化）
  - `file_id`（UUID）与 `chunk_hash`（hex）随机性强，压缩率较差
- 备份的 `index` 阶段会把整个 SQLite 文件 zstd 压缩后分片上传到 Telegram：
  - index 上传耗时可能远大于 data upload，表现为“卡住/带宽没吃满”
  - 多机器场景下 `index_sync` 也需要下载/写入巨大 DB，成本过高
- 观察：`chunks` / `chunk_objects` 作为去重映射规模相对可控；真正爆炸的是历史快照的 `files` / `file_chunks`。

## 目标 / 非目标

### Goals

- 在**不改变**现有加密/manifest 协议（`IndexManifest` v1）的前提下，把 remote index 的体积从
  - `O(keep_last_snapshots * files_per_snapshot)`
  - 降到 `O(sources * files_latest_per_source)`
- 仍然保证：
  - `index_sync` 下载最新 remote index 后，可用于下一次备份的 base-chunk-copy（只需要每个 source 的 latest 快照文件映射）
  - 任意旧快照仍可恢复：通过其自身的 `manifest_object_id` 下载对应 remote index（该 index 在生成时包含当时的 latest 文件映射，即包含该快照）
- 运行时不增加 daemon 内存峰值（仍保持“流式压缩 + 分片上传”）。

### Non-goals

- 改造本地 index schema（例如 path 字典化、`chunk_hash` 改 BLOB、delta snapshots）
- 改 `IndexManifest` 版本/格式（v2）或增加额外 remote objects
- 一次性解决“本地 index 文件长期膨胀”的全部问题（可作为后续 maintenance 任务）

## 范围（Scope）

### In scope

- core：在 `crates/core/src/backup.rs` 的 `upload_index` 中，上传前生成一个“compact export DB”：
  - 全量复制：`snapshots` / `chunks` / `chunk_objects` / `remote_indexes` / `remote_index_parts` / `tasks`
  - 裁剪复制：`files` / `file_chunks` 仅包含“每个 `source_path` 的 latest snapshot”的数据
  - 对 export DB 进行 zstd + 加密分片上传（沿用现有 upload_index 逻辑）
- 新增单元测试验证 export DB 的保留/裁剪规则（覆盖多 source、多 snapshot）。
- 增加日志：输出 export 前后 DB 大小（bytes）与保留 snapshot 数，便于排障与验证收益。

### Out of scope

- 自动在本地 DB 上执行同样裁剪（如需要，另开 spec 或作为可选 CLI/maintenance 命令）

## 需求（Requirements）

### MUST

- restore/verify 对任意 snapshot：
  - 下载该 snapshot 的 `manifest_object_id` 对应 remote index 后，`files` / `file_chunks` 必须包含该 snapshot
- index_sync 下载 latest remote index 后：
  - 对每个 `source_path`，`files` / `file_chunks` 至少包含该 source 的 latest snapshot（保证 base-chunk-copy 工作）
- 不引入新的 Telegram 协议/限制风险：仍沿用现有分片上传路径与重试策略。

### SHOULD

- latest remote index 的字节数显著下降（通常约为旧方案的 `~1/keep_last_snapshots`，取决于 source 数量）
- index 阶段耗时显著下降

### COULD

- 后续补充本地 maintenance：对本地 index 执行同样裁剪并触发 VACUUM，从而缩小磁盘占用

## 功能与行为规格（Functional/Behavior Spec）

### Core flows

- backup run 生成新 snapshot `S`
- `upload_index`：
  - 从本地 index DB 生成 compact export DB `E`
  - 对 `E` 做 zstd 流式压缩，分片加密上传，生成 `IndexManifest`（v1）
  - 将 manifest 上传并返回 object id（bootstrap 仍只存 `manifest_object_id`）

### Edge cases / errors

- 多 source_path：每个 source 都保留各自 latest snapshot 的 `files` / `file_chunks`
- 空目录/无文件：允许 `files` / `file_chunks` 为空，但 snapshot 行必须存在
- 任何 export 失败都应使 backup 失败并打印错误（避免上传一个“缺表/缺行”的 index）

## 接口契约（Interfaces & Contracts）

None

## 验收标准（Acceptance Criteria）

- Given 一个 index DB 含两个 `source_path`，各自 2 个 snapshot（共 4 个 snapshot 行）
  When 生成 compact export DB
  Then export DB 仍包含 4 条 `snapshots` 元数据与全部 `remote_indexes`（若存在）
  And `files` / `file_chunks` 只包含两个 source 的 latest snapshot
- Given latest remote index 被 `index_sync` 写入本地
  When 对其中一个 source 再跑一次 backup 且文件未变化
  Then base-chunk-copy 逻辑仍可命中（不会重新 chunk 整棵树的所有文件）
- Given 旧 snapshot 的 `manifest_object_id`
  When 下载其 remote index 并 restore
  Then restore 仍可从该 DB 找到该 snapshot 的 `files` / `file_chunks` 并完成恢复

## 实现前置条件（Definition of Ready / Preconditions）

- 规格已明确“裁剪规则（按 source latest）”与恢复/同步不变式
- 单测覆盖多 source、多 snapshot 的裁剪正确性

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: `cargo test -p televy-backup-core`
- Full workspace: `cargo test --all-features`

### Quality checks

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --all-features -- -D warnings`

## 文档更新（Docs to Update）

- `docs/specs/dyu56-index-tiered-filemaps/SPEC.md`: 本规格
- `docs/specs/README.md`: Index 表新增条目并跟踪状态

## 计划资产（Plan assets）

None

## 资产晋升（Asset promotion）

None

## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: 在 `upload_index` 中实现 compact export DB（按 source latest 裁剪 files/file_chunks）
- [x] M2: 添加单测覆盖裁剪规则
- [ ] M3: 真机验证：观察 index upload bytes 与耗时下降，且 index_sync + 下一次 backup 正常

## 方案概述（Approach, high-level）

- remote index 文件的“爆炸”来自历史快照的 `files` / `file_chunks` 重复存储；但运行时关键路径（base-chunk-copy）只需要每个 source 的 latest 快照映射。
- 因此在上传前进行 export：把“全局去重映射与快照目录（chunks/chunk_objects/snapshots/remote_indexes）”保留为全量，把“文件映射（files/file_chunks）”裁剪为每个 source 的 latest。
- 每个旧快照在其自身 remote index 中仍包含其文件映射（因为当时它是 latest），所以 restore 仍可按 manifest 单独下载。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：若未来新增“离线浏览旧快照文件列表”的 UI 功能，下载 latest index 将不再包含旧快照文件映射；届时需按需下载对应 snapshot 的 remote index。
- 风险：export 过程会增加一些本地 SQLite I/O（copy rows）；但相比减少 GB 级上传通常仍是净收益。
- 假设：restore/verify 始终以 snapshot 的 `manifest_object_id` 为入口下载对应 remote index（不依赖 latest index 内含旧快照 files/file_chunks）。

## 变更记录（Change log）

- 2026-03-02: 创建规格

## 参考（References）

- 本地大库样本：`/Users/ivan/Library/Application Support/TelevyBackup/index/index.ep_*.sqlite`（multi-GB）
- Schema：`crates/core/migrations/0001_init.sql`
