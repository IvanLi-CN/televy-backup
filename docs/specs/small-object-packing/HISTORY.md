# History

## Provenance

- Legacy source: `docs/plan/0002:small-object-packing/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/specs/telegram-backup-mvp/contracts/file-formats.md`: 如采用 pack，需要补充/调整 “chunk 上传对象” 的文件格式与体积策略说明。
- `docs/specs/telegram-backup-mvp/contracts/db.md`: 如采用 pack，需要补充/调整 `chunk_objects.object_id` 的编码约定或引入新表。
- `docs/requirements.md`: 补充“上传对象归并策略”（pack 的启用条件与体积约束等，便于用户理解备份行为）。


## 方案概述（Approach, high-level）

- 核心思路：把“要上传的对象”从“每个 chunk 一个对象”提升为“多个 chunk → 一个 pack 对象”。
- 启用条件：当待上传对象数量/体积超过阈值时启用 pack；小规模变更允许直接上传独立对象（减少读放大）。
- 打包策略：使用简单贪心装箱（按生成顺序向当前 pack 追加；soft target 为 32MiB；hard max 为 49MiB）。
- 安全性：pack 内仍然只包含加密后的 chunk blob（以及加密的 pack header）；不泄露 chunk hash 等敏感元数据。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0002:small-object-packing/PLAN.md`.
