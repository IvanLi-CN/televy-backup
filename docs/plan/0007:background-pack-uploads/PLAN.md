# Pack 上传后台并发（scan 与 upload 解耦）（#0007）

## 状态

- Status: 待实现
- Created: 2026-01-22
- Last: 2026-01-22

## 已冻结决策（Decisions, frozen）

- 队列上限采用“双阈值”，并以“等待回压”为唯一策略：
  - `max_pending_jobs = max_concurrent_uploads * 2`
  - `max_pending_bytes = max_concurrent_uploads * PACK_MAX_BYTES * 2`
- `min_delay_ms` 按“全局上传启动速率”执行（所有 worker 共享同一个节流器；避免并发导致速率叠加）。
- 速率限制覆盖 backup 内所有 `upload_document`（pack + 直传 blob + index parts/manifest）。

## 背景 / 问题陈述

- 目前 backup 的 scan 阶段会在遍历文件时触发 pack flush，并同步调用 `storage.upload_document(...)`，导致 scan 阶段被网络上传串行阻塞。
- 在网络抖动/上行带宽不足场景下，会出现“scan 看似很慢、但实际上卡在上传”的现象，影响吞吐与可排查性。
- 需要把 pack 上传从 scan 里解耦，改为后台并发上传（在资源约束下做有界并发与回压），让 scan 的耗时主要由本地 IO/CPU 决定。

## 目标 / 非目标

### Goals

- scan 阶段不再等待 Telegram 上传：scan 的主循环不应 `await storage.upload_document(...)`。
- upload 阶段支持后台并发上传（worker pool），并提供可控的速率限制（并发数 + 最小间隔）。
- 保持正确性口径不变：去重、索引一致性、失败重试语义（任务失败后下次可重跑）、取消（CancellationToken）行为明确。
- 保持内存可控：上传队列为有界队列（capacity/bytes budget），必要时对 scan 施加回压。

### Non-goals

- 不修改 pack 尺寸策略（`PACK_TARGET_BYTES`/`PACK_MAX_BYTES`）与启用阈值策略（`PACK_ENABLE_MIN_OBJECTS` 等）。
- 不引入新的远端存储后端，不改变 MTProto helper 的协议。
- 不承诺绝对的上传吞吐（受网络/Telegram 侧限制），只保证“scan 不被上传串行阻塞”的结构性改善。
- 不在本计划内做“落盘队列/磁盘 spool”（如需要，另立计划）。

## 范围（Scope）

### In scope

- 调整 `televy_backup_core::backup` 的备份管线：
  - scan 阶段只负责：遍历文件、分块、加密封装、写入索引（SQLite）。
  - upload 阶段负责：把 scan 产出的 pack（以及必要时的 direct blob）通过后台 worker 并发上传，并回写 `chunk_objects` 映射。
- 以 `config.toml` 中现有的 `telegram_endpoints[].rate_limit.*` 作为上传速率限制的来源（使用当前 backup target 绑定的 endpoint）。
- 补齐测试：用可控延迟的 fake storage 覆盖“scan 不等待上传”“队列回压”“失败传播”关键路径。

### Out of scope

- 新增 UI 配置项（例如“上传并发/队列上限可配置”）或新的 CLI 参数。
- 变更 SQLite schema（除非实现过程中发现必须，否则保持不动）。

## 需求（Requirements）

### MUST

- scan 阶段不得直接上传：在遍历文件与 chunking 循环内，不允许调用 `storage.upload_document(...).await`。
- 必须支持 pack 后台上传：当 pack 达到 flush 条件时，允许在 scan 阶段 finalize pack bytes，但上传必须交由后台 worker 执行。
- 必须处理“大 blob 直传”场景：当单 blob 超过 pack 预算时，仍需能上传（但上传同样必须后台化）。
- 必须有界：后台上传队列必须有明确上限（以条目数或字节预算表达）；达到上限时 scan 必须施加回压（阻塞/等待）而不是无限制占用内存。
- 并发与速率限制必须可控：使用当前 endpoint 的 `rate_limit.max_concurrent_uploads` 与 `rate_limit.min_delay_ms` 来约束 upload worker 的行为。
- 失败语义：任一上传失败应导致本次 backup 失败，且错误信息可定位（至少包含 object kind / bytes / 原因）；失败后下次重跑允许重新上传缺失对象，不破坏索引一致性。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `config.toml`：`telegram_endpoints[].rate_limit.*` | Config | external | Modify | ./contracts/config.md | app/core | CLI / daemon / macOS app | 本计划仅落地“上传侧”的并发/节流语义（download 侧不在本计划范围内） |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/config.md](./contracts/config.md)

## 验收标准（Acceptance Criteria）

- Given 一个 storage 实现，其 `upload_document` 人为引入固定延迟（例如 2s/次），且待上传 pack 数量足够多，
  When 执行一次 backup，
  Then `phase.finish(scan)` 的耗时不应随“单次上传延迟”线性增长（scan 不等待上传；如触发队列回压则按回压规则阻塞）。
- Given 当前 target 的 endpoint 满足 `rate_limit.max_concurrent_uploads = 2`，
  When 执行一次 backup 且产生多个 pack 上传，
  Then 同时进行的上传任务数不超过 2（可通过测试用假 storage 统计并发峰值验证）。
- Given 当前 target 的 endpoint 满足 `rate_limit.min_delay_ms = 250`，
  When 连续触发多次上传，
  Then 上传启动间隔满足最小间隔约束（按契约定义的口径验证）。
- Given 任意一次上传失败，
  When 本次 backup 结束，
  Then 任务应失败并返回可定位错误；下次重跑可继续完成备份且不会产生同一 `chunk_hash` 的不一致引用。

## 实现前置条件（Definition of Ready / Preconditions）

- 目标/非目标、范围（in/out）已明确
- 验收标准覆盖 core path + 队列回压 + 失败传播
- 契约已定稿（见 `./contracts/config.md`），实现与测试可直接按契约落地
- 关键取舍已由主人确认（见 Blockers）

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: uploader 队列回压（capacity/bytes budget）与速率限制口径（min delay）测试（tokio time 控制）。
- Integration tests: 以 fake storage 注入延迟与失败，覆盖：
  - scan 不等待上传（在不触发回压前）
  - 触发回压后的行为符合契约
  - 上传失败能终止任务并给出错误上下文

### Quality checks

- 按仓库既有约定执行 lint/typecheck/格式化/静态检查（不引入新工具）。

## 文档更新（Docs to Update）

- `docs/architecture.md`: 补充“备份管线分阶段（scan/upload/index）”以及 upload worker/队列/回压的高层说明。
- `docs/plan/0003:sync-logging-durability/PLAN.md`: 如 scan/upload 阶段语义发生变化（scan 不再包含上传等待），在 Notes 或验收口径中补充说明。

## 实现里程碑（Milestones）

- [ ] M1: 设计并落地 upload 队列与 worker pool（有界队列 + 并发上限 + 最小间隔）
- [ ] M2: 改造 backup scan：pack/direct blob 产出 → enqueue（scan 内不 await 上传）
- [ ] M3: upload 阶段 drain 队列并回写 `chunk_objects`；错误传播与取消语义对齐
- [ ] M4: 补齐测试（延迟/失败/回压/并发上限）并在本地跑通
- [ ] M5: 更新文档（`docs/architecture.md`）并补充可观测口径（scan/upload 分离后的含义）

## 方案概述（Approach, high-level）

- scan 阶段把“需要上传的 payload（pack bytes / direct blob bytes）”封装成 job，通过有界队列交给后台 worker；worker 负责上传并产出“已上传对象引用”结果。
- 主流程在 upload 阶段等待所有 job 完成，聚合统计并将结果回写到 SQLite（`chunk_objects`），最后进入 index/retention。
- 速率限制：以当前 endpoint 的 `rate_limit.max_concurrent_uploads` 控制并行度；以 `rate_limit.min_delay_ms` 控制上传启动节奏（具体口径见契约）。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：scan 与 upload 并发写 SQLite 可能引入锁竞争（需在实现中控制写入路径，必要时引入单独的 DB 写入通道）。

- 假设：本计划不新增新的配置字段，只复用 v2 settings 中 endpoint 的 `rate_limit.*`（单次 backup 取当前 target 绑定的 endpoint）。
- 假设：不引入额外的 per-target rate_limit override；如未来需要跨 endpoint 的全局节流策略，再单独调整契约与实现。

## 参考（References）

- 本仓库日志：`~/Library/Application Support/TelevyBackup/logs/sync-backup-*.ndjson`（可用于对比 scan/upload 分离前后耗时分布）。
