# Targets 主界面与执行记录（按目标聚合 backup/restore/verify）（#kaa5e）

## 状态

- Status: 待实现
- Created: 2026-01-29
- Last: 2026-01-29

## 背景 / 问题陈述

- 当前 macOS app 只有菜单栏浮窗（popover）与 Settings window，缺少一个可以按 Target 浏览/追溯执行记录（备份/恢复/校验）的主界面。
- 用户需要：按 Target 展示历史摘要，并能在主界面直接触发恢复与校验。

## 目标 / 非目标

### Goals

- 新增主界面窗口：
  - 列表展示所有 Targets；
  - 每行提供 `Restore…` 与 `Verify` 按钮；
  - 点击 Target 行进入详情页，按该 Target 聚合展示 `backup/restore/verify` 的历史摘要记录。
- 菜单栏浮窗的设置按钮改为“进入主界面”按钮。
- 主界面提供 Settings 入口，并支持快捷键打开 Settings（沿用 `⌘,`）。
- `restore/verify` 的 run log 补齐 `target_id`，保证“按目标显示执行记录”可用且一致。

### Non-goals

- 不在本计划中重做 daemon 调度逻辑（restore/verify 仍为手动触发）。
- 执行记录 UI 不展示完整日志逐行明细，仅展示摘要（可提供“打开日志文件”入口）。
- 不实现“选择历史 snapshot”的恢复/校验（仅支持 latest）。

## 范围（Scope）

### In scope

- macOS app：
  - 新增主界面窗口与路由（Targets 列表 / Target 详情）；
  - 目录选择对话框：Restore 必须选择空目录（非空则阻止并提示）；
  - 调用 CLI 触发 `restore latest` 与 `verify latest`，并处理 `--events` 输出（task.state/progress）以更新 UI 状态/Toast；
  - 本地解析 per-run NDJSON 日志，并按 `target_id` 聚合展示历史摘要（最近 N 条，倒序）。
- CLI：
  - `verify latest`（按 `--target-id` 或 `--source-path` 选择 target，并 resolve latest snapshot 后执行 verify）；
  - `restore latest` 与 `verify latest` 的 `run.start/run.finish` tracing 字段补齐 `target_id`（建议同时包含 `endpoint_id/source_path`）。

### Out of scope

- 远端/本地索引与存储协议变更。
- 执行记录持久化索引数据库（优先目录扫描与轻量缓存；如确有性能问题再单独立计划）。

## 需求（Requirements）

### MUST

- 主界面 Targets 列表：
  - 数据源：以 status snapshot 中的 targets 为准（包含 id/source_path/endpoint_id/enabled/state/lastRun）。
  - 每行按钮：
    - `Restore…`：弹出目录选择；若目标目录非空则提示并取消；若为空则开始 `restore latest`。
    - `Verify`：直接开始 `verify latest`。
- Restore latest：
  - 调用 CLI：`televybackup --events restore latest --target-id <id> --target <dir>`
  - 任务进行中与完成时 UI 有可见反馈（running/phase/progress/ok/error）。
- Verify latest：
  - 调用 CLI：`televybackup --events verify latest --target-id <id>`
  - 任务进行中与完成时 UI 有可见反馈（running/phase/progress/ok/error）。
- 执行记录（摘要）：
  - 从 per-run NDJSON 日志提取每次 run 的元信息（kind/run_id/startedAt/finishedAt/status/duration/error_code/bytes/files…），并按 `target_id` 分组展示；
  - 旧日志若缺少 `target_id`，归类到 `Unknown target`（并不阻断主流程）。
- 菜单栏浮窗：
  - 右上角设置按钮替换为“进入主界面”按钮（打开并激活主界面窗口）。
- Settings：
  - 主界面提供 Settings 入口；
  - `⌘,` 打开 Settings window（保持现有行为）。

### SHOULD

- Targets 列表每行显示该 target 最近一次 `backup/restore/verify` 的简要状态（若可得）。
- 历史记录列表提供“打开日志文件”动作，便于排障。

## 验收标准（Acceptance Criteria）

- Given 已存在至少一个 target，
  When 用户点击浮窗“进入主界面”，
  Then 主界面窗口出现并展示 Targets 列表。

- Given Targets 列表中某个 target，
  When 用户点击 `Restore…` 并选择一个非空目录，
  Then 恢复不会开始，UI 明确提示“目录必须为空”。

- Given Targets 列表中某个 target，
  When 用户点击 `Restore…` 并选择一个空目录，
  Then 开始执行 `restore latest`，UI 显示运行中；任务完成后，执行记录中出现一条 `restore` 摘要，且该记录包含 `target_id` 并被正确归档到该 target。

- Given Targets 列表中某个 target，
  When 用户点击 `Verify`，
  Then 开始执行 `verify latest`；任务完成后，执行记录中出现一条 `verify` 摘要，且该记录包含 `target_id` 并被正确归档到该 target。

- Given 主界面可见，
  When 用户按下 `⌘,`，
  Then Settings window 打开（或聚焦到已有 Settings window）。

## 非功能性验收 / 质量门槛（Quality Gates）

- UI 不应在主线程做大规模日志扫描（需要后台线程/增量扫描策略）。
- 日志解析失败不应影响主界面可用性（失败时降级为“无记录/部分记录”并提示）。
- 不把运行/交付依赖挂在 `docs/plan/` 下。

## 文档更新（Docs to Update）

- `README.md`：补充“从主界面触发 restore/verify（latest）”的简要说明（如需要）。

## 实现里程碑（Milestones）

- [ ] M1: macOS 主界面窗口（Targets 列表/详情路由）与浮窗入口调整
- [ ] M2: Restore UI（空目录校验 + 触发 restore latest + 任务反馈）
- [ ] M3: Verify UI（触发 verify latest + 任务反馈）
- [ ] M4: CLI `verify latest` + `restore/verify` run log 补齐 `target_id`
- [ ] M5: 执行记录摘要聚合展示（按 target 分组 + 最近 N 条）与端到端验收

## 风险与开放问题

- 日志体量增长：目录扫描可能变慢，需要增量缓存或上限策略（默认仅展示最近 N 条）。
- 任务并发：UI 侧需明确“同一时间仅允许一个任务运行”的行为与提示（沿用现有 isRunning 语义）。

## Change log

- 2026-01-29: 使用 `docs-plan-id` 生成 `kaa5e`。
