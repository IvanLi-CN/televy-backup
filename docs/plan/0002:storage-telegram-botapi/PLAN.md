# Telegram Bot API 存储适配层（#0002）

## 状态

- Status: 待实现
- Created: 2026-01-18
- Last: 2026-01-18

## 背景 / 问题陈述

需要一个稳定的 Telegram Bot API 存储适配层，承载“上传/下载/复用/限速/重试”这些与 provider 强绑定的能力，避免其逻辑泄漏到 chunking/index/restore 等核心模块。

## 目标 / 非目标

### Goals

- 提供 `telegram.botapi` provider 的最小可用适配层：上传 chunk part、下载 chunk part、上传索引 part/manifest、下载索引 part/manifest。
- 具备可控的并发、限速与重试策略，且错误可归类为 retryable/non-retryable。
- 支持对象复用（优先通过 `file_id` 复用下载路径；上传侧可缓存并复用“已上传对象”的引用）。

### Non-goals

- 变更 Telegram 接入通道（仍固定 Bot API）。
- 处理 Telegram 账号体系/风控策略的“外部不可控”问题（只做缓解与提示）。

## 范围（Scope）

### In scope

- Bot API 交互：上传（sendDocument 等）、下载（getFile + file download URL）、健康检查（getMe / 权限检查）。
- 对象引用：以 `file_id` 作为下载主键（并在本地索引记录必要映射）。
- 失败与重试：退避、429 处理、网络错误重试、幂等性设计（避免重复上传/重复写索引）。

### Out of scope

- Webhook 形态（不依赖 webhook）。

## 需求（Requirements）

### MUST

- 能上传“≤50MB 的文件对象”（parts / manifest / chunk blobs）并返回稳定引用（`file_id`）。
- 能基于 `file_id` 下载文件内容，并校验 hash。
- 对常见错误进行分类：unauthorized/forbidden/chat_not_found/rate_limited/network 等。
- 支持代理配置（若用户环境需要）。
- 并发与限速可配置（避免触发 Telegram 风控）。
- 对上传做“本地去重”与“断点续传友好”（例如先查 DB 是否已存在 object_id）。

## 接口契约（Interfaces & Contracts）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| None | - | - | - | - | - | - | 本交付项不新增/修改对外接口；按 #0001 契约实现 |

## 验收标准（Acceptance Criteria）

- Given 已配置 bot token（Keychain）与私聊 `chat_id`，When 执行 `telegram_validate`，Then 返回 bot username 与 chatId，且错误可明确分类。
- Given 任意一个 ≤50MB 的测试文件，When 上传后再下载，Then 内容 hash 一致且可重复下载。
- Given 模拟 429/网络抖动，When 上传或下载，Then 适配层能按策略重试，并向上层暴露 retryable 标志与等待时间（若 Bot API 返回可解析信息）。

## 质量门槛（Quality Gates）

- 单元测试覆盖：错误分类、重试/退避、hash 校验。
- 集成测试覆盖：在 CI 中使用“假 Telegram 服务（local mock）”跑一次 upload→download round-trip；不在 CI 内跑真实 Telegram（避免 secrets 泄漏与不稳定）。

## 里程碑（Milestones）

- [ ] M1: Bot API client（含代理/超时/重试基建）
- [ ] M2: Upload/Download primitives（parts + manifest）
- [ ] M3: Error taxonomy + retry policy（可观测、可测试）

## 风险与开放问题

- 风险：Telegram 风控导致短期/长期不可用；需在 UI 与日志中暴露清晰提示。
