# Chunking 分块上限调整（按存储模式 + 内存预算）（#0006）

## 状态

- Status: 待实现
- Created: 2026-01-22
- Last: 2026-01-22

## 已冻结决策（Decisions, frozen）

- 存储模式：仅 MTProto（本仓库为 MTProto-only）。
- 上传单文件工程上限：`MTProtoEngineeredUploadMaxBytes = 128MiB`（定义为单次 `upload_document` 的 bytes 上限，包含 framing）。
- chunk 上限：`chunking.max_bytes <= 128MiB - 41 bytes`（framing 开销固定 41 bytes）。
- pack 大小：
  - `PACK_MAX_BYTES = 128MiB`
  - `PACK_TARGET_BYTES = 64MiB ± 8MiB`（56–72MiB，按 pack 做 jitter）
  - `PACK_MAX_ENTRIES_PER_PACK = 32`
  - `PACK_ENABLE_MIN_OBJECTS = 10`（保持不变）
- UI：暂不提供可配置项（通过默认值与实现常量控制）。

## 背景 / 问题陈述

- 当前 chunking 的 `max_bytes` 在 `core` 层仍被一个“约 50MiB 量级”的 hard cap 限制，即便仓库已进入 **MTProto-only** 存储模式，也无法把 chunk 上限提高到更大的值。
- 现状会让大文件/大量文件场景产生更多 chunk object（更多上传次数、更多远端对象、更多索引条目），影响性能与可用性。
- 此外，当前 pack 的 hard max 也仍是 `49MiB`（历史上为避免触碰 50MB 量级上限），在 MTProto-only 下属于“人为偏小”的约束；即便 chunking 默认不变，提高 pack 上限/目标也能显著减少上传 document 数量。
- 同时：当前 chunking/encrypt/pack/upload 以“整块在内存中处理”为主，过大的 chunk 上限会带来显著的内存峰值与失败风险，需要一个工程化的上限与清晰的错误提示。

## 用户与场景（Users & Scenarios）

- 单用户 macOS：备份目录包含大文件（如照片库/虚拟机镜像/工程产物）。
- MTProto-only 模式下，希望减少 object 数量与上传次数（更快、更省资源）。

## 目标 / 非目标

### Goals

- 将“上传到 Telegram 的最大单文件尺寸”（本项目通过 MTProto 上传 document）固化为一个工程上限，并据此提升 `chunking.max_bytes` 的上限（建议默认：`128MiB`）。
- 上传文件尺寸需要“抖动”（jitter）：避免上传出的 document 大小过于规律（优先作用于 pack document；chunk 本身已由 CDC 自带波动）。
- 在配置校验错误中给出明确、可操作的提示（包含上限值与计算方式）。
- 在不改动 chunking 语义（CDC/hash/加密 framing）前提下，明确并固化：`chunking.max_bytes` 与“上传单文件工程上限”的关系（写入契约）。

### Non-goals

- 不在本计划内把 chunking/encrypt/upload 改成完全流式（streaming）管线。
- 不在本计划内重新设计 pack 策略（除非为兼容更大的 `max_bytes` 暴露出明确 bug）。
- 不在本计划内调整 `INDEX_PART_BYTES`（index 分片大小）除非主人明确要一并处理。

## 范围（Scope）

### In scope

- `core`：将 chunk size 校验从“固定 50MiB 量级上限”调整为“MTProto-only 的工程上限”（见契约）。
- `core`：将 pack 的 `PACK_MAX_BYTES` / `PACK_TARGET_BYTES` 调整为与 MTProto 工程上限一致的量级（减少上传 document 数量；不引入新 pack 格式版本）。
- `cli` / `daemon` / `macOS app`：默认配置与配置校验错误信息对齐新的上限规则（本计划不引入 UI 可配）。
- 文档：更新 `docs/architecture.md` 与本计划契约，清晰说明上限与风险。

### Out of scope

- 任何会改变运行行为的实现提交（本会话为 plan 阶段，后续进入 impl 才做）。

## 需求（Requirements）

### MUST

- `chunking.min_bytes/avg_bytes/max_bytes` 的基本约束必须保持：`> 0` 且 `min <= avg <= max`。
- `chunking.max_bytes` 的上限必须是**可解释**且**可测试**的规则：
  - MTProto：必须有一个明确的工程上限（小于 Telegram 理论上限），用于控制内存峰值、上传超时与失败面。
- 当 `chunking.max_bytes` 超限时：
  - `backup`/`restore`/`verify` 必须在开始阶段就失败（配置错误），并输出包含 “当前上限是多少/为什么” 的错误信息。
- 必须补齐测试覆盖：
  - 覆盖 MTProto 上限边界（刚好等于/超过 1 字节）。
  - 覆盖 `min/avg/max` 关系错误。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `config.toml` `[chunking]` 上限语义 | Config | internal | Modify | ./contracts/config.md | core/cli | app/daemon | 按存储模式解释 `max_bytes` 上限 |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/config.md](./contracts/config.md)

## 验收标准（Acceptance Criteria）

- Given `telegram.mode = "mtproto"` 且 `chunking.max_bytes > (MTProtoEngineeredUploadMaxBytes - 41 bytes)`
  When 启动 `backup run`（或 daemon scheduled backup）
  Then 立即失败为配置错误，错误信息包含 `MTProtoEngineeredUploadMaxBytes` 与 `41 bytes` 开销及计算口径说明
- Given `telegram.mode = "mtproto"` 且 `chunking.max_bytes == (MTProtoEngineeredUploadMaxBytes - 41 bytes)`
  When 启动 `backup run`
  Then 配置校验通过（不因 chunk size 被拒绝）
- Given `min/avg/max` 不满足 `min <= avg <= max`
  When 启动任意需要 chunking 的命令
  Then 立即失败为配置错误

## 实现前置条件（Definition of Ready / Preconditions）

- 目标/非目标、范围（in/out）、约束已明确
- 验收标准覆盖 core path + 关键边界/异常
- 接口契约已定稿（或明确 `None`），实现与测试可以直接按契约落地
- 关键取舍（尤其：MTProto 上限是多少、是否需要 UI 可配）已由主人确认

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: 覆盖 chunking 校验边界与错误信息的可读性（按仓库既有测试框架）。
- Integration tests: 如现有测试套件覆盖 backup pipeline，补齐 “大于 pack 上限的 chunk 仍可 direct upload” 的回归测试（仅当本计划实现触及相关逻辑）。

### Quality checks

- 现有 lint / fmt / typecheck 全部通过（不引入新工具）。

## 文档更新（Docs to Update）

- `docs/architecture.md`: 补充 chunking 上限与其与内存预算/稳定性的关系（避免文档口径与实际实现偏离）。

## 方案概述（Approach, high-level）

- 在 `core` 层把 “上传单文件上限” 固化为 MTProto-only 的工程上限（`128MiB`，本计划已冻结），并据此放开 `chunking.max_bytes` 上限（扣除 framing `41 bytes`）。
- 工程上限以“上传 document 的 bytes”定义，并显式纳入 framing 开销（`+41 bytes`）；因此可接受的 `chunking.max_bytes` = `engineered_upload_max_bytes - 41`。
- pack 策略（兼顾“减少上传次数”与“不要一个 pack 塞太多小文件”）：
  - `PACK_MAX_BYTES = 128MiB`（与工程上限一致）
  - `PACK_TARGET_BYTES = 64MiB`（主人已定）
  - `PACK_TARGET_BYTES` 引入抖动：每个 pack 的 flush 阈值在 `64MiB ± jitter` 区间内变化（实现不引入新依赖，使用现有 `blake3` 做确定性 jitter）
- 将该规则固化在 `./contracts/config.md`，并为边界写单测。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：把 chunk 上限设得过大会显著抬高内存峰值（chunk data + 加密后 blob + packer carry + MTProto helper 读入缓冲），在低内存机器上可能导致 OOM/明显卡顿。
- 风险：chunk 越大，单次上传耗时越长；当前 helper 对 `upload_stream` 有 30 分钟封顶超时，慢链路更容易触发失败（从而放大重试成本）。
- 假设：工程上限默认取 `128MiB`（定义为“单次 `upload_document` bytes 上限”，包含 framing）。

### 对比：200MiB（不推荐作为默认）

- 内存峰值（粗估）：
  - `core` 进程：chunk data（≤200MiB）+ 加密后 blob（≤200MiB）+ packer（通常约 56–72MiB；最坏上界为 `PACK_MAX_BYTES=128MiB`）→ ~450–530MiB 量级
  - `mtproto-helper` 进程：会再读入一份上传 bytes（≤200MiB）→ +200MiB
  - 合计（两进程）：~650–730MiB+（不含 OS/运行时开销）
- 上传超时：
  - helper 超时上限为 30 分钟；200MiB 在 30 分钟内完成需要约 114KiB/s 的有效上传速率（更慢会更易超时）

### 合理性评估（128MiB）

- 内存峰值（粗估，跨进程合计）：
  - 128MiB：~400–500MiB+（core ~300MiB 量级 + helper ~128MiB + 开销）
- 上传超时（30 分钟封顶）：
  - 128MiB：需要约 73KiB/s 的有效上传速率

### 抖动建议（jitter）

- 只对 pack 的 flush 阈值做抖动即可（chunk 本身由 CDC 天然不固定；index part 继续保持固定大小，避免引入格式复杂度）。
- 建议区间：`PACK_TARGET_BYTES = 64MiB ± 8MiB`（即 56–72MiB）。
- 选择 64MiB 的原因：restore/verify 在读取 `tgpack:*` slice 时需要下载**整个 pack document**（仅缓存最近一个 pack），因此 pack 太大将放大“单次 miss 需要下载的冗余 bytes”；64MiB 作为 128MiB hard max 的一半，在“减少上传次数”与“限制恢复侧冗余下载/内存峰值”之间取中间值。

### “不要一个 pack 里塞太多小文件”（Owner preference A）

由于 pack 的 header 会按 entry 增长，且 restore/verify 侧会整包下载并缓存，**entry 数过多**会带来：

- header 变大、解析与内存占用更高
- 小文件 restore 时更容易出现“下载很多无关 bytes 才拿到其中一个 slice”的体验

因此建议在保持 `PACK_TARGET_BYTES = 64MiB` 的同时，再引入一个 **entries 上限**：

- `PACK_MAX_ENTRIES_PER_PACK = 32`：当 entries 达到 32 就强制 flush（即使还没到 target bytes）

取值影响（32 entries 的代价）：

- `PACK_TARGET_BYTES = 64MiB` 仍可能在“小文件极多”的场景被 entries cap 先触发，但相比 10 entries，64MiB target 仍更有机会主导 pack 大小。
- 会增加 pack documents 数量（相对“无 entries cap”或 cap 很大），但不会像 10 entries 那么激进。
- 好处：限制单 pack 的 entries 数量上限，避免 pack header/restore 侧整包下载的放大效应。

## 参考（References）

- Telegram 官方：Telegram Premium（2GB/4GB 上传能力）作为 MTProto 方向的能力参考。
