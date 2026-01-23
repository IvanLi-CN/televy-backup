# UI Components Contracts（macOS app）

> Kind: UI Component（external）

## 1) Popover: remove Logs

- Scope: external
- Change: Modify

### Requirement

- Popover 内不得出现 `Logs` Tab，也不得出现日志列表界面（不提供日志浏览能力）。
- 其余 Popover 导航结构（是否保留 `Settings` Tab、是否改为齿轮打开独立 Settings window）以计划 #0005 的最终口径为准；本计划仅冻结“移除 Logs”这一点。

### Behavior（语义）

- Popover 定位为“快速状态/入口”，不承载日志浏览与排查能力。

## 2) Settings: Open logs

- Scope: external
- Change: Modify

### UI

- 在 Settings 中提供一个明确的按钮入口：`Open logs`（名称实现阶段可微调，但语义需清晰）。

### Action

- 点击后打开日志目录（Finder），该目录必须覆盖：
  - per-run logs（`sync-*.ndjson`）
  - UI log（`ui.log`）
- 目录选择口径与 `file-formats.md` 一致（`TELEVYBACKUP_LOG_DIR` 优先，否则 `TELEVYBACKUP_DATA_DIR/logs/`，macOS 默认 `~/Library/Application Support/TelevyBackup/logs/`）。

### Failure semantics

- 若目录不存在或无法打开：best effort；需要以 toast/提示文案告知用户（具体文案实现阶段定）。
