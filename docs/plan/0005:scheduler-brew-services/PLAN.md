# 调度与后台常驻（brew services）（#0005）

## 状态

- Status: 待实现
- Created: 2026-01-18
- Last: 2026-01-18

## 背景 / 问题陈述

需要支持 hourly/daily 的无人值守备份，同时保持用户可控（启停、查看状态、查看日志）。已确认采用 `brew services` 以用户级 `LaunchAgent` 方式管理后台常驻进程，由后台进程按配置触发备份任务，App 不必常驻。

## 目标 / 非目标

### Goals

- 后台常驻：可通过 `brew services start/stop` 管理。
- 定时触发：hourly/daily 触发备份（至少支持“每小时一次”和“每天一次固定时间点”）。
- 幂等与互斥：同一时刻最多一个备份任务；避免重入。

### Non-goals

- 系统级 daemon（`LaunchDaemon`）不做。
- 复杂的作业编排（MVP 先做单 job）。

## 范围（Scope）

### In scope

- 后台进程的职责边界（触发、记录任务、上报结果）。
- 日志路径与基本轮转策略（避免无限增长）。
- 与 UI 的协作：UI 打开时可读到最近任务状态/结果（通过 SQLite / event replay）。

### Out of scope

- 多设备/多主机协调调度。

## 需求（Requirements）

### MUST

- 支持 hourly/daily 两种 schedule。
- 互斥：若上一次任务未结束，本次触发必须跳过并记录原因（不排队、不重入）。
- 可观测：至少记录每次触发的开始/结束时间与结果（写入 `tasks` 表）。
- 支持手动触发（“立即运行一次”）并写入任务记录。

## 接口契约（Interfaces & Contracts）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Schedule config | Config | internal | Modify | `0001:telegram-backup-mvp/contracts/file-formats.md` | core | app | schedule 字段由后台进程消费 |

## 验收标准（Acceptance Criteria）

- Given schedule=hourly，When 后台进程运行 2 小时，Then 至少触发 2 次备份（除非互斥跳过，并有记录）。
- Given schedule=daily 且设置了时间点，When 到达时间点，Then 触发一次备份并在 `tasks` 中可追溯。
- Given 用户执行 `brew services stop ...`，When 后台停止，Then 不再触发新任务。

## 质量门槛（Quality Gates）

- 单元测试覆盖：schedule 计算、互斥锁、任务落库。

## 里程碑（Milestones）

- [ ] M1: 后台进程形态与生命周期（启动/停止/日志）
- [ ] M2: schedule 计算与触发
- [ ] M3: 互斥与任务落库
