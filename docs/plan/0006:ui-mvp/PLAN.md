# UI MVP（#0006）

## 状态

- Status: 待实现
- Created: 2026-01-18
- Last: 2026-01-18

## 背景 / 问题陈述

MVP 需要一个可用的桌面 UI：用户能配置 Telegram、选择源目录、发起备份/恢复/校验、查看进度/统计/错误，并能在失败时知道下一步怎么做。

## 目标 / 非目标

### Goals

- 任务流：Backup / Restore / Verify 的发起、进行中状态、完成结果展示。
- 设置页：Bot token（写入 Keychain）、chat_id、chunking 参数、schedule 参数（后台进程消费）。
- 可观测：进度条、吞吐、去重率、耗时；错误码映射到用户可读信息。

### Non-goals

- 高度美化/复杂交互（不做）。
- 多语言与主题系统（不做）。

## 范围（Scope）

### In scope

- 页面：Dashboard（概览/最近任务）、Backup（选择源/运行）、Restore（选择 snapshot/目标）、Verify、Settings。
- 事件订阅：消费 `task:state` 与 `task:progress` 事件，按 `taskId` 幂等更新。

### Out of scope

- 除 `tauri-driver` WebDriver E2E 之外的额外 UI 自动化体系（不引入）。

## 需求（Requirements）

### MUST

- 能显示 secrets presence（不回显 secret 内容）。
- 能发起任务并展示其状态与错误。
- 能展示最基本的统计数据（snapshots/chunks/bytes）。
- 支持取消任务与重试。

## 接口契约（Interfaces & Contracts）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| UI ↔ Backend | RPC/Event | internal | New | `0001:telegram-backup-mvp/contracts/rpc.md` / `events.md` | app | web | 本计划按既有契约实现 |

## 验收标准（Acceptance Criteria）

- Given 未配置 token，When 打开 Settings，Then UI 引导用户写入 token 并可验证连通性。
- Given 备份任务运行中，When UI 打开 Dashboard，Then 能看到实时进度与阶段（phase）更新。
- Given 任务失败，When 查看详情，Then UI 显示可读错误信息与“下一步操作”（重试/检查网络/检查 chat 权限）。

## 质量门槛（Quality Gates）

- E2E tests（required）：必须覆盖以下最小用户路径（以 CI 的 Linux runner 上可稳定跑通为准）：
  - Settings：写入 Bot token（E2E 固定启用 `TELEVYBACKUP_E2E=1`，后端 secrets provider 使用 in-memory 实现，不调用系统 Keychain）
  - Validate：执行一次连通性校验并展示结果
  - Backup：发起一次备份并看到 progress/state 变化，最终进入 succeeded/failed
  - Error path：模拟限速/网络失败时，UI 能展示可读错误与“可重试/不可重试”

备注：Tauri 官方文档指出 macOS 桌面缺少 WKWebView driver，WebDriver E2E 目前仅 Windows/Linux 可用；因此 E2E 固定在 CI 的 Linux runner 上执行。

E2E 技术栈固定为 WebdriverIO + `tauri-driver`，在 CI 的 Linux runner 上执行。

另外：关键状态机（task state/phase）在前端必须有单元测试（使用 Vitest），覆盖状态迁移与事件乱序/重复的幂等处理。

## 里程碑（Milestones）

- [ ] M1: Settings（token presence + 写入 + validate）
- [ ] M2: Task views（backup/restore/verify 基本页面）
- [ ] M3: Progress & errors（events + 错误映射）
