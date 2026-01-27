# 备份远端索引权威 + 本地自动同步（remote-first）（#0012）

## 状态

- Status: 待实现
- Created: 2026-01-27
- Last: 2026-01-27

## 背景 / 问题陈述

- 当前备份流程以本地索引 `TELEVYBACKUP_DATA_DIR/index/index.sqlite` 为真源：scan 会依赖本地 DB 判断 chunk 是否已存在（去重/复用）。
- 这导致一个“自动化失败点”：用户清理数据/重装/迁移到新设备后，本地索引丢失，备份无法自然续跑增量（会退化为大量重复上传）。
- 系统已经具备远端“索引权威”的基础：每次备份都会上传整份加密压缩的 SQLite（分片 + manifest），并且用 pinned 的 bootstrap catalog 记录 latest 指针（用于 `restore latest`）。

结论：应将“远端 latest 索引”作为权威真源；本地索引只是可丢的缓存副本。备份在开始 scan 前，应自动将本地索引与远端 latest 对齐（必要时下载并原子替换）。

## 目标 / 非目标

### Goals

- 备份前置自动化：`backup run` 启动后，在进入 scan 之前完成“远端 latest → 本地索引对齐”（remote-first correctness + local-first performance）。
- 让“重装/新设备继续增量备份”成为默认路径：只要用户持有 master key（TBK1）且 pinned catalog 存在。
- 提供可控开关：允许显式禁用远端对齐（例如离线场景），但默认启用。
- 安全性：避免因 master key 不匹配导致误覆盖 pinned catalog（见下文 Requirements）。

### Non-goals

- 不做远端 GC（Telegram chat 里的对象回收）。
- 不做跨设备并发冲突解决（两设备同时备份导致 latest 指针频繁变化，只保证“每次备份前尽量对齐最新”）。
- 不把“远端索引”改成可查询服务（仍以“整库 SQLite 上传/下载”作为索引交换单位）。

## 范围（Scope）

### In scope

- CLI：`backup run` 增加“索引对齐”前置步骤（默认启用）；必要时提供手动命令。
- Core：抽取并复用“download remote index db（manifest → sqlite）”能力，用于备份前置对齐。
- Bootstrap：对 pinned catalog 的覆盖策略做安全收紧（避免错误 key 时静默覆盖）。
- 事件/进度：增加一个新的 phase（例如 `index_sync`），并保证对现有 UI/解析器的向后兼容（additive）。

### Out of scope

- GUI UI/UX 大改（除非为暴露新错误码/新 phase 的最小适配；具体在 impl 阶段评估）。

## 需求（Requirements）

### MUST

#### 1) 备份前置：远端 latest → 本地索引对齐

- 对齐依据：从 pinned bootstrap catalog 解析出 `latest.snapshot_id + latest.manifest_object_id`（按 target_id 优先；否则按 source_path 匹配的既有策略）。
- 对齐触发（任一满足即可）：
  - 本地 `index/index.sqlite` 不存在；
  - 本地存在但“不包含远端 latest”或“包含但 manifest_object_id 不一致”（视为 stale/corrupt）。
    - 判定方法：本地 DB 的 `remote_indexes` 表中，必须存在 `snapshot_id = remote_latest.snapshot_id` 且 `manifest_object_id = remote_latest.manifest_object_id` 的记录（并且 `provider` 必须与当前 endpoint provider 一致）；否则视为 stale。
- 对齐动作：
  - 下载远端 index manifest + parts，解密解压后生成 SQLite 文件；
  - 以原子方式替换 `index/index.sqlite`（写入临时文件 → rename 覆盖），避免半写状态；
  - 对齐完成后才允许进入 scan。
- 当 pinned catalog 不存在（bootstrap missing）：视为“首次备份/未启用跨设备指针”，允许直接用本地索引（若不存在则新建）继续备份。

#### 2) 可控开关

- CLI 提供禁用远端对齐的开关：`--no-remote-index-sync`。
- 默认行为：开启远端对齐（remote-first）。

#### 3) pinned catalog 覆盖安全

- 当 pinned catalog 存在但无法解密时（可能是不同 master key 或非 TelevyBackup 文档）：
  - 默认不得静默覆盖 pinned 指针；
  - 本计划内不提供“覆盖 pinned”的入口：此时直接报错并提示用户导入正确 master key（TBK1）。如未来确有需求，再单独出计划提供显式覆盖能力（高风险）。

### SHOULD

- 提供手动命令：`televybackup index sync ...`（用于用户显式把本地索引恢复到远端 latest，或排障时强制拉取）。若本计划实现范围不足，可推迟到后续计划。
- 进度/日志：在 `--events` 模式下，能看到 `index_sync` 的 start/finish 以及下载字节数、耗时等信息。

### MUST NOT

- 不得因为“无法解密 pinned catalog”而自动清空/覆盖它（除非用户显式确认）。
- 不得把 secrets 写入日志或事件输出。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `televybackup backup run` | CLI | internal | Modify | ./contracts/cli.md | CLI | GUI / daemon | 默认 remote-first index sync |
| `task.progress.phase`（新增 `index_sync`） | Event | internal | Modify | ./contracts/events.md | core/CLI | GUI | additive-only |
| pinned bootstrap catalog 行为 | Behavior | internal | Modify | （本计划内描述） | core | CLI/GUI | 覆盖策略收紧 |

### 契约文档

- [contracts/README.md](./contracts/README.md)
- [contracts/cli.md](./contracts/cli.md)
- [contracts/events.md](./contracts/events.md)

## 兼容性与迁移（Compatibility / migration）

- 对老用户：
  - 若本地索引存在且与远端 latest 一致：备份行为不变（额外成本仅为一次轻量的“检查远端 latest 指针”）。
  - 若 pinned catalog 存在但不可解密：备份前置将失败并提示修复（而不是覆盖）。这属于“更安全但更严格”的行为变更，需要在 release notes 里提示。

## 验收标准（Acceptance Criteria）

- Given pinned bootstrap catalog 存在且可解密，并且本地 `index/index.sqlite` 不存在，
  When 执行 `televybackup backup run ...`，
  Then 在进入 scan 前自动下载远端 latest 索引并落地为 `index/index.sqlite`，随后备份正常继续。

- Given pinned bootstrap catalog 存在且可解密，但本地索引落后于远端 latest，
  When 执行 `backup run`，
  Then 自动对齐到远端 latest 后再扫描，并确保去重行为以远端 latest 为准（不出现“索引缺失导致的大量重复上传”）。

- Given pinned bootstrap catalog 存在但不可解密，
  When 执行 `backup run`（默认行为），
  Then 不覆盖 pinned catalog，并返回可操作的错误（提示导入正确 master key / TBK1）。

- Given 用户显式禁用远端对齐（例如 `--no-remote-index-sync`），
  When 执行 `backup run`，
  Then 不访问 pinned catalog，不下载远端索引，按本地索引执行（若本地索引不存在则按首次备份处理）。

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认 CLI flag：`--no-remote-index-sync`（默认不传则启用远端对齐）。
- 已确认“远端 latest 检查”策略：每次备份启动都读取 pinned catalog；仅当本地缺失/判定 stale 时才下载远端索引并替换本地。
- 已确认 pinned catalog 解密失败时的策略：默认阻断，不覆盖 pinned；用户应导入正确 master key（TBK1）。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit: 远端 latest 与本地 index 状态判定（missing/stale/match）。
- Integration: 使用 mock storage/pinned storage 验证：
  - 本地缺失 → 自动下载并替换；
  - pinned 不可解密 → 默认拒绝覆盖；
  - `--no-remote-index-sync` → 不触发下载。

### Reliability

- 本地索引写入必须原子替换，避免写一半导致 DB 损坏。
- 失败路径必须可重试（不进入 scan；不破坏既有本地索引）。

## 文档更新（Docs to Update）

- `docs/architecture.md`：补充“remote-first index sync”的备份前置步骤与失败语义（实现阶段同步）。
- `README.md`：补充“重装/新设备继续备份”的推荐流程与开关说明（实现阶段同步）。

## 实现里程碑（Milestones）

- [ ] M1: Core 抽取并复用“download remote index db（manifest → sqlite）”能力（支持原子落盘）
- [ ] M2: CLI `backup run` 接入 preflight index sync（默认启用 + 开关）
- [ ] M3: pinned catalog 覆盖策略收紧（decrypt 失败默认拒绝覆盖 + 明确错误指引）
- [ ] M4: 测试覆盖（unit + integration）
- [ ] M5: 文档与 release notes 更新

## 风险与开放问题（Risks / Open Questions）

- 远端索引体积增长：整库下载可能变慢；是否需要后续计划做“增量索引”或“分层索引”（不在本计划内）。
- 两设备交替备份：latest 指针可能频繁变化；是否需要“备份开始时锁定 head，并在结束时更新 latest”策略（当前倾向：开始时对齐最新即可）。
