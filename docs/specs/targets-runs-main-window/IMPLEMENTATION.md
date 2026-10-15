# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 非功能性验收 / 质量门槛（Quality Gates）

- UI 不应在主线程做大规模日志扫描（需要后台线程/增量扫描策略）。
- 日志解析失败不应影响主界面可用性（失败时降级为“无记录/部分记录”并提示）。
- 不把运行/交付依赖挂在 `docs/plan/` 下。


## 实现里程碑（Milestones）

- [x] M1: macOS 主界面窗口（Targets 列表/详情路由）与浮窗入口调整
- [x] M2: Restore UI（空目录校验 + 触发 restore latest + 任务反馈）
- [x] M3: Verify UI（触发 verify latest + 任务反馈）
- [x] M4: CLI `verify latest` + `restore/verify` run log 补齐 `target_id`
- [x] M5: 执行记录摘要聚合展示（按 target 分组 + 最近 N 条）与端到端验收

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/kaa5e:targets-runs-main-window/PLAN.md`.

## Direct Navigation Coverage

- Status identity fields are additive and optional for old daemon compatibility.
- Menu-bar navigation is keyed by target plus run/task identity; missing identity or matching logs deliberately falls back to the selected Target overview.
- Terminal navigation rejects run-start-only logs, and external task completion carries the known snapshot identity through status IPC.
- Daemon lifecycle helpers keep the explicit run and snapshot identity parameters at each terminal transition, including lint-safe wrappers for the longer status-update calls.
- Deterministic `ui_demo` capture remains the visual evidence source; live application windows are not captured.

## Failure and History Coverage

- Snapshot setup failures use one terminal recorder for the live status and per-run log; the original failure codes remain intact.
- The parser recognizes target-scoped legacy snapshot setup failures, probes the writer lock without blocking, and re-reads the tail before marking an abandoned log interrupted.
- Main-window summaries and history rows share plain-language failure descriptions. Target details offer Settings and Diagnostics actions.
- `BackupFailureHistoryTests` covers legacy recovery, explicit-terminal precedence, active-writer protection, interrupted rows, bounded diagnosis, and old status decoding. The daemon regression verifies one terminal event with matching status identity, code, reason, and duration.
- GitHub-hosted macOS CI uses SwiftUI `ImageRenderer` to render light and dark production failure-notice and history-row components from synthetic data with `BackupFailureHistoryPreview`; it does not capture an installed app or use backup data, FDA, or a privileged helper.

## Window Viewport Coverage

- Failure recovery text follows the proposed width and height of the production split view. It must not request an unbounded vertical ideal size that displaces the entire sidebar and detail above the window.
- `MainWindowLayoutTests` creates the production `NSWindow` with synthetic status and history stores, transitions from running to failed, and measures actual native view bounds against `contentLayoutRect`. Test-only background anchors expose layout geometry without changing Release rendering.
- The regression checks short and long history, scrolling to both ends, default and minimum sizes, enlargement/restoration, light/dark appearance, and History/Diagnostics switching. It verifies the heading, recovery actions and tab controls remain visible while history scrolls separately.
- `scripts/macos/test-main-window-layout.sh` runs on GitHub-hosted macOS in PR CI and through the manual main-window layout validation input of `ci-main.yml`. Captures are restricted to the synthetic test window; installed apps, owner logs, backup sources and privileged services are not used.
