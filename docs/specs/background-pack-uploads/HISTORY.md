# History

## Provenance

- Legacy source: `docs/plan/0007:background-pack-uploads/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/architecture.md`: 补充“备份管线分阶段（scan/upload/index）”以及 upload worker/队列/回压的高层说明。
- `docs/specs/sync-logging-durability/SPEC.md`: 如 scan/upload 阶段语义发生变化（scan 不再包含上传等待），在规范或验收口径中补充说明。


## 方案概述（Approach, high-level）

- scan 阶段把“需要上传的 payload（pack bytes / direct blob bytes）”封装成 job，通过有界队列交给后台 worker；worker 负责上传并产出“已上传对象引用”结果。
- 主流程在 upload 阶段等待所有 job 完成，聚合统计并将结果回写到 SQLite（`chunk_objects`），最后进入 index/retention。
- 速率限制：以当前 endpoint 的 `rate_limit.max_concurrent_uploads` 控制并行度；以 `rate_limit.min_delay_ms` 控制上传启动节奏（具体口径见契约）。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0007:background-pack-uploads/PLAN.md`.
