# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认“状态弹出界面”的具体指代（Popover vs 其他状态弹窗），以及移除范围。
- 已确认 UI 日志文件的路径与是否需要对齐 `TELEVYBACKUP_*_DIR` 环境变量口径（见契约）。
- 已确认 “Open logs” 打开日志目录（单一入口）。
- 验收标准覆盖 core path + 日志落盘失败场景（best effort）。
- 关键文件与入口点已定位（见下方 References）。


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Manual smoke (macOS): 运行 `scripts/macos/build-app.sh` 构建并启动 App，验证 Popover UI 与 `ui.log` 落盘。
- Regression: 不影响既有 Rust CI（`cargo fmt --check` / `cargo clippy` / `cargo test`）。

### Quality checks

- 按仓库既有约定执行 Rust 侧质量检查（不引入新工具）。


## 实现里程碑（Milestones）

- [x] M1: 移除 Popover 的 `Logs` Tab（导航与布局对齐，确保无悬挂入口）
- [x] M2: 固化并验证 UI 日志落盘（路径/格式/脱敏/失败 best effort）与契约一致
- [x] M3: 更新文档（`README.md` / `docs/architecture.md`）说明日志位置与排查路径

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0008:status-popup-file-logging/PLAN.md`.
