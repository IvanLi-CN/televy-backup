# 小对象打包降低 Bot API 调用频率（Pack）（#0002）

## 状态

- Status: 已完成
- Created: 2026-01-19
- Last: 2026-01-20

## 已冻结决策（Decisions, frozen）

- 主方案：采用 pack（把多个加密 chunk blob 聚合成更少的上传对象），作为“减少 Bot API 调用次数”的主要手段。
- pack 启用条件：当“待上传对象”满足下列任一条件时启用 pack；否则允许直接以独立对象上传（避免为少量变更引入不必要的读放大）：
  - 待上传对象数量 `> 10`
  - 待上传对象总字节数 `> 32MiB`
- pack 尺寸策略（固定常量，不暴露为 `config.toml`）：
  - 目标尺寸（soft target）：`32MiB`
  - 最大尺寸（hard max）：`50MiB - 1MiB = 49MiB`（避免顶到上限）
  - 推导：在上述约束下，若单个 blob ≤ `49MiB - 32MiB = 17MiB`，则它可以安全追加到一个“已达到目标尺寸”的 pack 而不超过 hard max。
- 读放大（read amplification）：接受（恢复少量 chunk 可能需要下载整个 pack；后续再通过 pack 尺寸与缓存策略平衡）。

## 背景 / 问题陈述

- 备份数据在进入 Telegram 之前通常会经历分块（chunking）与加密封装；当源目录包含大量小文件时，会产生大量小 chunk。
- 若每个 chunk 都对应一次（或少量）Bot API 上传调用，将导致调用频率过高、整体吞吐降低，并更容易触发限速/失败重试风暴。
- 需要一种“低实现成本/低运行开销”的归并策略，把多个小对象聚合成更少的上传单元，同时保持去重、可恢复与可校验。

## 目标 / 非目标

### Goals

- 显著降低“上传数据对象”阶段对 Telegram Bot API 的调用次数（特别是小文件/小 chunk 场景）。
- 保持现有数据安全口径：chunk 仍然以加密形式存储，索引不泄露明文。
- 保持去重能力：同一 `chunk_hash` 在仓库中已存在时不重复上传。
- 可恢复与可校验：恢复/校验流程能定位并读取 pack 内的 chunk 数据。

### Non-goals

- 不追求最优的装箱/聚类算法（使用简单的贪心策略即可）。
- 不依赖 Telegram HTTP Range/分段下载能力（如后续需要，可另立计划）。
- 不引入新的远端存储后端（仍以 Telegram Bot API 为主）。

## 用户与场景（Users & Scenarios）

- 个人用户在 macOS 上对一个包含大量小文件的目录做周期性备份（例如 Time Machine 备份盘中的元数据与小对象）。
- 用户期望备份过程稳定、失败可恢复，并且不会因为大量小对象导致“API 调用风暴”。

## 需求（Requirements）

### MUST

- 必须提供“归并上传单元”的能力：把多个小 chunk 聚合成更少的上传对象（pack），以减少 Bot API 调用次数。
- pack 的大小必须受 Bot API 限制约束（给定 Telegram 官方 Bot API 默认上传限制时，需保证安全余量）。
- pack 相关参数不提供用户级配置项（不新增 `config.toml` 可配置开关/阈值）；以实现常量固定并在文档中冻结口径。
- pack 必须支持从中定位并读取单个 chunk（至少通过本地索引可定位 offset/len）。
- 必须保持 chunk 级去重：已有 `chunk_hash` 不再被打入新的 pack 上传。
- 必须支持失败重试与断点续传语义（至少做到“pack 上传失败可重试，不会导致重复写入同一 chunk 记录”）。
- 必须提供可观测统计：至少能输出/记录本次备份中“新增上传对象数、归并前后调用次数估算/对比”。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Telegram Bot API（上传/下载） | HTTP API | external | Modify | ./contracts/http-apis.md | storage | backup/restore/verify | 以 `sendDocument` 上传 pack（document）；以 `getFile` 下载 |
| Pack 文件格式（远端对象） | File format | internal | New | ./contracts/file-formats.md | core | storage/index | pack 作为“上传对象”聚合单位 |
| SQLite schema（chunk 定位信息） | DB | internal | Modify | ./contracts/db.md | core | backup/restore/verify | 需要能表达“pack + offset/len” |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/http-apis.md](./contracts/http-apis.md)
- [contracts/file-formats.md](./contracts/file-formats.md)
- [contracts/db.md](./contracts/db.md)

## 验收标准（Acceptance Criteria）

- Given 一个包含大量小文件（例如 1 万个、单文件 4–64KiB）的目录，
  When 执行一次备份并产生新增数据上传，
  Then “上传数据对象”的 Bot API 调用次数应接近 `ceil(新增数据字节数 / 32MiB)` 的量级（而不是与文件数同阶）。
- Given 本次备份产生的“待上传对象”数量 ≤ 10 且总大小 ≤ 32MiB，
  When 执行一次备份并产生新增数据上传，
  Then 允许不启用 pack（直接上传独立对象），且调用次数 ≤ 10。
- Given 任意一个快照，
  When 执行 restore，
  Then 能成功从 pack 中读取所需 chunk 并正确重组文件，且校验通过。
- Given 备份过程中任意一次上传失败并触发重试，
  When 任务重启继续，
  Then 不会出现同一 `chunk_hash` 被重复写入远端引用（本地索引保持一致）。
- Given Bot API 上传限制（默认官方服务器），
  When 生成 pack，
  Then pack 的上传文件大小不超过约束且留有余量（考虑加密封装开销）。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: pack 装箱策略（边界：刚好装满/溢出/最后一个 pack）、pack header 编解码、offset/len 计算正确性。
- Integration tests: 使用“假存储（mock storage）”统计上传调用次数，覆盖：大量小文件、混合大小文件、失败重试。

### Quality checks

- 按仓库既有约定执行 lint/typecheck/格式化/静态检查（不引入新工具）。

## 文档更新（Docs to Update）

- `docs/plan/0001:telegram-backup-mvp/contracts/file-formats.md`: 如采用 pack，需要补充/调整 “chunk 上传对象” 的文件格式与体积策略说明。
- `docs/plan/0001:telegram-backup-mvp/contracts/db.md`: 如采用 pack，需要补充/调整 `chunk_objects.object_id` 的编码约定或引入新表。
- `docs/requirements.md`: 补充“上传对象归并策略”（pack 的启用条件与体积约束等，便于用户理解备份行为）。

## 实现里程碑（Milestones）

- [x] M1: 实现 pack writer/reader（含 header）与单元测试
- [x] M2: 存储适配层支持 pack 上传/下载（`sendDocument`/`getFile`）
- [x] M3: SQLite schema / object_id 编码调整与迁移策略落地
- [x] M4: Backup 管线接入 pack（统计归并收益、失败重试路径）
- [x] M5: Restore/Verify 管线接入 pack（正确性与性能基线）

## 方案概述（Approach, high-level）

- 核心思路：把“要上传的对象”从“每个 chunk 一个对象”提升为“多个 chunk → 一个 pack 对象”。
- 启用条件：当待上传对象数量/体积超过阈值时启用 pack；小规模变更允许直接上传独立对象（减少读放大）。
- 打包策略：使用简单贪心装箱（按生成顺序向当前 pack 追加；soft target 为 32MiB；hard max 为 49MiB）。
- 安全性：pack 内仍然只包含加密后的 chunk blob（以及加密的 pack header）；不泄露 chunk hash 等敏感元数据。

## 风险与开放问题（Risks & Open Questions）

### 风险

- 放大读取（read amplification）：恢复少量 chunk 可能需要下载整个 pack（需通过 pack 尺寸与缓存策略权衡）。
- 出错恢复复杂度：pack 损坏会影响其中多个 chunk（需要校验与修复策略）。

### 开放问题（需要主人决策）

None

## 假设（Assumptions，需要确认）

- Telegram 官方 Bot API 上传限制与行为以官方文档为准；实现不依赖未明确承诺的 Range 下载能力。
- 备份/恢复以“索引可用”为前提；pack header 主要用于校验与辅助修复（而非替代索引）。

## 参考（References）

- Telegram Bot API：Sending Files / `sendDocument` / `getFile`：
  - https://core.telegram.org/bots/api#sending-files
  - https://core.telegram.org/bots/api#senddocument
  - https://core.telegram.org/bots/api#getfile
- 参考实现思路：restic pack files（pack 包含多个加密 blob，header 位于末尾）：
  - https://raw.githubusercontent.com/restic/restic/master/doc/design.rst
- 参考实现思路：Git pack/idx（打包 + 索引以减少大量小对象开销）：
  - https://git-scm.com/docs/git-pack-objects
  - https://git-scm.com/docs/pack-format
