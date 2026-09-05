# History

## Provenance

- Legacy source: `docs/plan/0005:multi-backup-directories-keyed-restore/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/architecture.md`：更新 Known limitations（新增跨设备 restore 机制），补充 bootstrap/catalog 与多 endpoint 设计。
- `docs/specs/telegram-backup-mvp/contracts/file-formats.md`：同步 config schema（v1 → v2）与 bootstrap/catalog 说明。
- `README.md`：增加“金钥备份/迁移”与“新设备恢复”指引。


## Change log

- 2026-01-22: 实现 settings v2（targets/endpoints）、TBK1 金钥导入/导出、pinned bootstrap/catalog、`restore latest`，并完成 macOS Settings window + 文档同步。
- 2026-01-22: 订正 Settings window 设计图，使其与当前实现一致（toolbar segmented control、Targets 侧栏宽度与底部 +/- 控制条、字段布局）。


## UI 设计（Design）

- Popover 现有 UI 基准：`docs/design/ui/liquid-glass-popover-overview.png`（及同名 `.svg`）
- Settings window（Targets）：[design/settings-window-targets.png](../../plan/0005:multi-backup-directories-keyed-restore/design/settings-window-targets.png)（source: [svg](../../plan/0005:multi-backup-directories-keyed-restore/design/settings-window-targets.svg)）
- Settings window（Recovery Key / 金钥）：[design/settings-window-security.png](../../plan/0005:multi-backup-directories-keyed-restore/design/settings-window-security.png)（source: [svg](../../plan/0005:multi-backup-directories-keyed-restore/design/settings-window-security.svg)）
- Settings window（Schedule）：[design/settings-window-schedule.png](../../plan/0005:multi-backup-directories-keyed-restore/design/settings-window-schedule.png)（source: [svg](../../plan/0005:multi-backup-directories-keyed-restore/design/settings-window-schedule.svg)）
- Popover（移除 Settings tab + gear 打开 Settings window）：[design/popover-minimal.png](../../plan/0005:multi-backup-directories-keyed-restore/design/popover-minimal.png)（source: [svg](../../plan/0005:multi-backup-directories-keyed-restore/design/popover-minimal.svg)）
- 浏览器测量预览：`docs/plan/0005:multi-backup-directories-keyed-restore/design/_preview-settings-window.html` / `docs/plan/0005:multi-backup-directories-keyed-restore/design/_preview-popover-minimal.html`

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0005:multi-backup-directories-keyed-restore/PLAN.md`.
