# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 测试 / 验证

- Dev：运行 `./scripts/macos/run-app.sh`，确认 `.app` 名称、bundle id、menubar 图标。
- Prod（手工）：设置 `TELEVYBACKUP_APP_VARIANT=prod` 后运行 `./scripts/macos/build-app.sh` 并启动，确认仍为 `TelevyBackup.app` 与 prod bundle id。


## 里程碑（Milestones）

- [x] M1: `build-app.sh` / `run-app.sh` 支持 dev/prod 变体（bundle id/name 分离）
- [x] M2: dev menubar 图标叠加 `DEV` 徽标
- [x] M3: 主人验收：dev+prod 可并存运行，且 `DEV` 徽标清晰可辨

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/3ejpg:macos-dev-app-variant/PLAN.md`.
