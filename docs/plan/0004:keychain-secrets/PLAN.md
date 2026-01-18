# macOS Keychain 密钥与凭据管理（#0004）

## 状态

- Status: 待实现
- Created: 2026-01-18
- Last: 2026-01-18

## 背景 / 问题陈述

本项目需要在本机安全保存 Bot token 与加密主密钥，且不得写入明文配置文件或日志；因此必须在 macOS 上采用 Keychain 做 secrets 存储，并提供可用的写入/读取/擦除与 presence 检查能力。

## 目标 / 非目标

### Goals

- Keychain 存储：Bot token、master key（或 master key 的封装密钥材料）。
- RPC 交互：前端可检查 secrets 是否已配置、可写入/更新、可擦除（擦除需确认）。
- 失败可诊断：区分“无权限/不可用/写入失败/读取失败”等错误。

### Non-goals

- 跨机同步 secrets（不做）。
- 复杂的密钥轮换策略（MVP 可先不支持 rotate）。

## 范围（Scope）

### In scope

- Keychain item 的命名与隔离（service/account 命名规则）。
- Presence API：不暴露 secret 内容给前端，只暴露是否已配置。
- 擦除流程：需要二次确认（UI 层做确认；后端做幂等删除）。

### Out of scope

- iCloud Keychain 同步策略（不依赖、也不阻止；以系统行为为准）。

## 需求（Requirements）

### MUST

- Bot token 与主密钥仅存于 Keychain。
- `settings_get` 返回 `*Present` 标志位（不回显 secret）。
- `settings_set` 可写入/更新 token（不落盘）。
- 错误码稳定且前端可展示为用户可理解的信息。

## 接口契约（Interfaces & Contracts）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Secrets presence & write | RPC | internal | Modify | `0001:telegram-backup-mvp/contracts/rpc.md` | app | web | 本计划交付实现；不额外新增契约文件 |

## 验收标准（Acceptance Criteria）

- Given Keychain 未配置，When 打开设置页，Then UI 显示“未配置 token/密钥”，且不会泄露任何 secret。
- Given 用户输入 token 并保存，When 再次打开设置页，Then 显示 token 已配置，且配置文件未出现 token 明文。
- Given Keychain 不可用或权限受限，When 保存 token，Then 前端收到明确错误码并可提示用户采取行动。

## 质量门槛（Quality Gates）

- 单元测试覆盖：Keychain wrapper 的错误映射与 presence 行为；测试固定使用 in-memory secrets provider（不调用系统 Keychain）。

## 里程碑（Milestones）

- [ ] M1: Keychain wrapper（get/set/delete + error mapping）
- [ ] M2: RPC 接入（presence + write flow）
- [ ] M3: UI 设置流程与错误提示
