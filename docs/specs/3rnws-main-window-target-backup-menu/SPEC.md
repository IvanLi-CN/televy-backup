# Main Window：Targets 菜单补齐 “Backup now”（#3rnws）

## 状态

- Status: 待实现
- Created: 2026-03-05
- Last: 2026-03-05

## 背景 / 问题陈述

- 当前 Main Window 的 Targets 行右键菜单仅有 `Restore…` / `Verify`，缺少“立即备份”的入口。
- 用户期望在不打开 Popover 的情况下，对某个 target 直接触发一次手动备份。

## 目标 / 非目标

### Goals

- 在 Main Window 提供**按 target** 的 `Backup now` 菜单项：
  - Toolbar 的 `Actions` 菜单（ellipsis）；
  - Targets 列表行的右键菜单（context menu）。
- `Backup now` 触发语义：对当前 target 执行 CLI `backup run`（manual label）。
- 即使 target 标记为 `Disabled`，仍允许手动 `Backup now`（不以 `enabled` 为门禁）。

### Non-goals

- 不引入新的 Rust schema/IPC/file-format 变更。
- 不修改 Popover 的 “Backup now (all enabled targets)” 行为（`control/backup-now`）。
- 不新增快捷键或额外 toolbar 按钮（仅补齐菜单入口）。

## 范围（Scope）

### In scope

- `macos/TelevyBackupApp/MainWindow.swift`：
  - Toolbar `Actions` menu 添加 `Backup now`；
  - `TargetListRow.contextMenu` 添加 `Backup now`。
- `macos/TelevyBackupApp/TelevyBackupApp.swift`：
  - 新增 `AppModel.backupRun(targetId:)` 并复用现有 CLI 执行管线与 toast/run-history 刷新。

### Out of scope

- CLI/daemon 调度策略修改（仍按既有 backup pipeline 执行）。

## 需求（Requirements）

### MUST

- 文案与顺序：
  - 菜单项文本：`Backup now`
  - 顺序：`Backup now`、分隔线、`Restore…`、`Verify`
- `Backup now` 的 gating：
  - 当 `model.isRunning == true`（或同等 busy 状态）时禁用；
  - 不得基于 `target.enabled` 禁用。
- 行为：
  - 点击触发 `televybackup --events backup run --target-id <id> --label manual`
  - 触发前 `ensureDaemonRunning()`
  - toast：`Starting backup…`
  - 进程退出后刷新 run history（与 restore/verify 一致）

## 验收标准（Acceptance Criteria）

- Given Main Window 中存在任意 target，
  When 用户打开 toolbar 的 `Actions` 菜单，
  Then 能看到 `Backup now`，且顺序正确。

- Given Main Window 的 Targets 列表任意一行，
  When 用户右键打开 context menu，
  Then 能看到 `Backup now`，且顺序正确。

- Given 某 target 显示为 `Disabled`，
  When 用户点击 `Backup now`，
  Then 动作仍会触发（不被 `enabled=false` 阻止）。

## 非功能性验收 / 质量门槛（Quality Gates）

- `scripts/macos/swift-unit-tests.sh` 通过。
- `cargo fmt --all`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all-features` 通过。
- `scripts/macos/build-app.sh` 通过。

