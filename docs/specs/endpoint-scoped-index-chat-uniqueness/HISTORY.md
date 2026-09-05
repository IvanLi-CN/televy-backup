# History

## Provenance

- Legacy source: `docs/plan/r6ceq:endpoint-scoped-index-chat-uniqueness/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/architecture.md`：更新 index/remote index 的 “endpoint-scoped” 语义与路径约定
- 相关主题：`docs/specs/remote-first-index-sync/SPEC.md`（若接口/语义受影响需要同步）


## 资产晋升（Asset promotion）

None


## 变更记录（Change log）

- 2026-01-31: 冻结决策：采用方案 B（本地按 endpoint 拆分索引库）+ 禁止 chat_id 复用
- 2026-01-31: 冻结迁移策略：不拆分旧全局 `index.sqlite`；仅从下一次备份开始新建 per-endpoint 索引库
- 2026-01-31: 冻结清理策略：per-endpoint DB 可用后自动删除旧全局 `index.sqlite`（静默）

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/r6ceq:endpoint-scoped-index-chat-uniqueness/PLAN.md`.
