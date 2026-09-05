# History

## Provenance

- Legacy source: `docs/plan/0006:chunking-max-bytes/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

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


## Change log

- 2026-01-23: Implemented MTProto chunking cap (128MiB - 41), updated pack sizing (128MiB max, 64MiB±8MiB target, max 32 entries), and added tests + docs sync.

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0006:chunking-max-bytes/PLAN.md`.
