# 恢复与校验 MVP（#0007）

## 状态

- Status: 待实现
- Created: 2026-01-18
- Last: 2026-01-18

## 背景 / 问题陈述

备份系统的可用性不在“能上传”，而在“能恢复且能证明恢复正确”。因此需要实现 restore 与 verify 的最小闭环：从 Telegram 下载索引分片与 chunks，重组文件并校验一致性。

## 目标 / 非目标

### Goals

- Restore：给定 `snapshot_id`，恢复到目标目录并确保文件树可用。
- Verify：对 snapshot 做一致性/完整性检查（索引可读、parts/chunks 存在、hash 匹配、可重组）。
- 错误可操作：缺失 manifest/parts/chunks 时，给出明确错误与下一步操作。

### Non-goals

- “秒级恢复速度”优化（先正确再快）。
- APFS snapshot 一致性冻结（另起计划）。

## 范围（Scope）

### In scope

- 下载索引：manifest → parts → join/decrypt/decompress → open sqlite。
- 下载 chunks：按 `chunk_hash` 拉取并校验。
- 文件重组：按 `file_chunks` 顺序写回目标路径。
- 基础 metadata：恢复 `mtime` 与 `mode`（若索引中存在；未知值按 0 处理）。
 - 基础 metadata：恢复 `mtime_ms` 与 `mode`（`mode=0` 表示未知）。

### Out of scope

- ACL/xattr/资源叉等高级属性还原（不做）。

## 需求（Requirements）

### MUST

- restore 到空目录可成功，且校验通过（hash/size/文件数）。
- verify 能检测：缺失 parts、缺失 chunks、hash 不一致、索引损坏。
- restore/verify 可取消，并保证中途失败不会留下“伪成功”状态。

## 接口契约（Interfaces & Contracts）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Restore/Verify RPC | RPC | internal | New | `0001:telegram-backup-mvp/contracts/rpc.md` | app | web | 本计划按既有契约实现 |
| DB schema | DB | internal | New | `0001:telegram-backup-mvp/contracts/db.md` | core | app | 依赖 remote index refs |
| Index formats | File format | internal | New | `0001:telegram-backup-mvp/contracts/file-formats.md` | core | app | 依赖 manifest+parts |

## 验收标准（Acceptance Criteria）

- Given 任意 snapshot，When restore 到空目录，Then 文件树生成且 verify 通过。
- Given 缺失一个 index part，When verify，Then 报错包含缺失 part_no 或可定位信息。
- Given chunk hash 不一致，When verify，Then 报错并标记不可重试（数据损坏或密钥不匹配）。

## 质量门槛（Quality Gates）

- 集成测试：备份→上传→下载→恢复→校验 的最小端到端路径固定使用“本地假存储（in-memory / fs mock）”，在 CI 上稳定执行。

## 里程碑（Milestones）

- [ ] M1: 索引下载与重组（manifest+parts）
- [ ] M2: chunk 下载与文件重组
- [ ] M3: verify（完整性检查与错误归类）
