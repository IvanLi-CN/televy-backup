# History

## Provenance

- Legacy source: `docs/plan/0008:status-popup-file-logging/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `README.md`: 在 troubleshooting/日志段落补充 UI 日志文件 `ui.log` 的默认位置与用途（区分 per-run NDJSON 与 UI 日志）。
- `docs/architecture.md`: 补充“可观测性/日志”小节对 UI 日志位置的引用（如该文档已有相关章节，则只追加最小说明）。
- `docs/specs/sync-logging-durability/contracts/file-formats.md`: 补充一句“UI 日志文件 `ui.log` 与 per-run logs 同目录”的说明（保持边界清晰）。


## Change log

- 2026-01-24: 移除 Popover 日志 UI 与内存日志列表；`ui.log` 写入 logs 目录（与 per-run logs 同目录）；Settings “Open logs” 打开日志目录。


## 方案概述（Approach, high-level）

- UI：删去 `Tab.logs` 与对应视图入口，Popover 只保留“状态概览 + 设置”。
- 可观测性：把“排查入口”迁移到落盘文件；通过契约文档固定日志文件的路径与格式，并在 Settings 提供“打开日志目录”入口，避免实现阶段口径漂移。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0008:status-popup-file-logging/PLAN.md`.
