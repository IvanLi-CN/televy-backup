# History

## Provenance

- Legacy source: `docs/plan/0010:status-popover-dashboard/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/design/ui/statusbar-popover-ia.md`: 更新 Popover IA（移除 tabs/Logs；Overview 变更为“全局 network + 多 target 列表”；Dev 为独立窗口且入口在 Settings）。
- `docs/design/ui/README.md`: 增加本计划设计图入口与预览指引。


## 设计图与说明（Design assets）

- `docs/design/ui/statusbar-popover-dashboard/popover-overview.svg` / `docs/design/ui/statusbar-popover-dashboard/popover-overview.png`
- `docs/design/ui/statusbar-popover-dashboard/popover-overview-empty.svg` / `docs/design/ui/statusbar-popover-dashboard/popover-overview-empty.png`
- `docs/design/ui/statusbar-popover-dashboard/developer-window.svg` / `docs/design/ui/statusbar-popover-dashboard/developer-window.png`
- `docs/design/ui/statusbar-popover-dashboard/README.md`
- `docs/design/ui/statusbar-popover-dashboard/_preview-popover.html`


## 方案概述（Approach, high-level）

- UI 侧以“单一状态快照（StatusSnapshot）”渲染：Overview 与 Dev 均只依赖同一份快照（避免两套口径）。
- 数据侧采用 `status stream`：UI 启动一个长生命周期进程读取 NDJSON，按快照驱动渲染，避免轮询/抖动/电量开销。
- `status stream` 的快照来源为 daemon 落盘 `status.json`（见 `./contracts/file-formats.md`）；CLI 负责读取并输出统一的 NDJSON `status.snapshot`。
- 对“实时速率”采用滑动窗口计算：后端或 UI 任选其一，但必须保证稳定性与可测试（契约中明确）。


## Change log

- 2026-01-25：实现 `status.json`（daemon）+ `televybackup status get/stream`（CLI）+ Popover Overview（全局 network + targets）+ Developer window（原始字段 + activity + Copy JSON/Reveal/Freeze）；同步设计资产到 `docs/design/ui/` 并更新 IA 文档；验证：`cargo test`、`scripts/macos/build-app.sh`。
- 2026-01-25：Popover 打开时 best-effort 拉起 `televybackupd`：优先 `launchctl kickstart gui/<uid>/homebrew.mxcl.televybackupd`，无服务时回退为从 app bundle（或 PATH）直接启动；`scripts/macos/build-app.sh` 将 `televybackupd` 打进 `.app`，确保本地构建可自动拉起。
- 2026-01-25：对齐 Popover Overview 视觉基准图：NETWORK/updated 排版、Up/Down chip 样式、Targets list（badge/row/empty state）与滚动分隔线；并将 daemon/status stream 的 best-effort 启动前置到 app launch（无需先打开 popover）。
- 2026-01-26：订正设计基准图：Targets 行 `label`↔badge 间距统一（视觉约 10px）；右侧信息语义固定为“主行时间类 + 次行数值类”，避免不同状态下右侧含义乱跳，并同步到 IA 文档。
- 2026-01-26：新增 `Backup now`（立即备份）按钮：多 targets 策略冻结为“立即备份所有 enabled targets”；实现为 UI 写入 `$TELEVYBACKUP_DATA_DIR/control/backup-now`，daemon 轮询消费触发并执行备份。
- 2026-01-26：补齐“短任务可见性”：`lastRun` 增加 `filesIndexed`；Popover idle 次行在 `bytesUploaded=0` 但 `bytesDeduped>0` 时展示 `saved bytesDeduped`（可附带 files）；并在观测到新 `lastRun` 时弹 toast 提示完成/失败。修复 UI 启动 CLI 时 env 不一致问题（传递 `TELEVYBACKUP_CONFIG_DIR`/`TELEVYBACKUP_DATA_DIR`）；当 CLI 不可用时退化为低频轮询 `status.json`，避免面板空白/误判断开。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0010:status-popover-dashboard/PLAN.md`.
