# History

## Provenance

- Legacy source: `docs/plan/0011:daemon-status-ipc/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/specs/status-popover-dashboard/contracts/cli.md`: 如数据源策略对 UI 有可见影响，补充说明（实现阶段同步）。


## Change log

- 2026-01-26: 新增 daemon status IPC（NDJSON over Unix socket），CLI `status get/stream` 默认改走 IPC 并保留 `status.json` fallback；补充 IPC cadence 与首条快照时延测试；同步更新相关契约与架构文档。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0011:daemon-status-ipc/PLAN.md`.
