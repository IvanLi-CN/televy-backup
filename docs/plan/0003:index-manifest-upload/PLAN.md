# 索引分片与 manifest（#0003）

## 状态

- Status: 待实现
- Created: 2026-01-18
- Last: 2026-01-18

## 背景 / 问题陈述

索引（SQLite）需要“可上传、可下载、可重组、可校验”，且必须满足 Bot API 单次上传大小限制；因此需要为索引定义稳定的打包、加密、分片与 manifest 规则，确保恢复路径可持续。

## 目标 / 非目标

### Goals

- 固化索引打包管线：`index.sqlite` → compress → encrypt → split → upload → upload manifest。
- 固化“定位规则”：如何在 Telegram 私聊历史中找到某个 snapshot 的 manifest。
- 固化一致性检查：manifest 与 parts 的 hash/size 校验，发现缺失/损坏时的错误与修复动作。

### Non-goals

- 改变索引底层存储（仍为 SQLite）。
- 设计复杂的远端 GC（MVP 可只增不减，或只做本地引用计数）。

## 范围（Scope）

### In scope

- `index-part` 文件命名规则与大小上限策略（留余量）。
- `index-manifest`（加密上传）文件命名规则与内容结构版本化（`version` 字段）。
- 与 DB 的落地：记录 manifest 与 parts 的远端引用，便于恢复与校验。

### Out of scope

- 多仓库/多设备并发写（MVP 不做）。

## 需求（Requirements）

### MUST

- 能从远端完整恢复到 `index.sqlite`，且校验通过（hash 一致）。
- manifest/parts 规则具备向后兼容能力（允许新增字段；禁止破坏性变更）。
- 对缺失 parts/manifest 的场景给出明确错误与下一步操作（例如“重新下载 manifest”）。

## 接口契约（Interfaces & Contracts）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Index packaging | File format | internal | New | `0001:telegram-backup-mvp/contracts/file-formats.md` | core | app | 本计划不再引入新契约文件，复用 #0001 口径 |

## 验收标准（Acceptance Criteria）

- Given 任意一次备份完成，When 读取 DB 对应 snapshot 的远端索引引用，Then 能下载 manifest 与全部 parts 并重组出 `index.sqlite`。
- Given 某个 part 缺失或 hash 不匹配，When 执行 restore/verify，Then 报错包含可操作的下一步操作（重试/重新下载/检查权限）。

## 质量门槛（Quality Gates）

- 单元测试覆盖：split/join、manifest 校验、错误分类。
- 集成测试覆盖：完整 round-trip（upload parts+manifest → download → reassemble → open sqlite）。

## 里程碑（Milestones）

- [ ] M1: 本地打包（compress+encrypt+split）与解包（join+decrypt+decompress）
- [ ] M2: manifest 生成/校验与 DB 落地
- [ ] M3: 远端 round-trip 验证（与 Telegram 适配层对接）
