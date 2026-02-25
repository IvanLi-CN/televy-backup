# 统一进度条规范（含 Prepare 并行）与四处 UI 对齐（#z324m）

## 状态

- Status: 已完成
- Created: 2026-02-25
- Last: 2026-02-25

## 背景 / 问题陈述

- 当前 backup 进度条在主窗口与浮窗存在多套渲染逻辑，表现不一致。
- 现有进度口径在高去重场景下容易出现“看起来不动”，不符合真实进度感知。
- backup 前置准备阶段（远端索引同步 + 本地统计）未统一暴露为可观测阶段，导致 UI 难以准确表达“正在准备”。

## 目标 / 非目标

### Goals

- 统一 backup 进度条语义与视觉：
  - 仅 `prepare` 使用滚动样式（indeterminate）。
  - `scan/upload/index` 使用单条双段确定性进度（成功段 + 扫描段）。
- 统一进度口径：
  - 成功段：`(bytesUploaded + bytesDeduped) / sourceBytesTotal`。
  - 扫描段：`bytesRead / sourceBytesTotal`。
- 统一入口行为：CLI 手动 backup 与 daemon 定时 backup 都采用并行 Prepare。

### Non-goals

- 不改 restore/verify 的进度条语义。
- 不重做远端存储协议、GC、或 backup 主流程语义。
- 不改 Settings import/compare 等非 backup 进度控件。

## 范围（Scope）

### In scope

- `TaskProgress` / `status snapshot` / `control IPC` / CLI events 的 additive 字段扩展。
- backup Prepare 并行化：`index_sync` 与 `local_quick_stats` 并发执行。
- 主窗口与浮窗 backup 进度条统一组件化与样式统一。

### Out of scope

- restore/verify 进度与状态文案重构。
- 非 backup 相关 UI 组件视觉统一。

## 需求（Requirements）

### MUST

- Prepare 阶段并行执行：`index_sync` + `local_quick_stats`。
- 仅 prepare 使用 indeterminate 进度条。
- backup 确定性阶段统一显示双段进度。
- 进度字段变更保持 additive，旧客户端可容错。

### SHOULD

- 提供 prepare 子任务失败降级：统计失败不阻断主流程，索引同步失败沿用现有阻断语义。
- UI 阶段文案统一映射：`preflight/index_sync/prepare -> Preparing`。

### COULD

- Prepare 文案显示子任务完成计数（如 `1/2`）。

## 功能与行为规格（Functional/Behavior Spec）

### Core flows

- 用户触发 backup（CLI/daemon 任一入口）后先进入 prepare。
- prepare 内 `index_sync` 与 `local_quick_stats` 并行运行。
- prepare 完成后进入 `scan/upload/index`，UI 使用双段确定性进度条展示。

### Edge cases / errors

- `local_quick_stats` 失败：记录日志，UI 降级为 indeterminate（直到可用 totals 出现）。
- `index_sync` 失败：保持现有错误码与阻断行为（例如 `bootstrap.decrypt_failed`）。
- 旧 snapshot 无新增字段：UI 不崩溃并回退兼容逻辑。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `TaskProgress` 新增 source totals 字段 | internal | internal | Modify | N/A | core | daemon/cli/ui | additive-only |
| `status.snapshot.targets[].progress` 新增字段 | events | internal | Modify | N/A | daemon/cli | macOS UI | additive-only |
| `status.taskProgress` IPC 新增字段 | rpc | internal | Modify | N/A | core/daemon | CLI -> daemon | additive-only |
| `task.progress` NDJSON 新增字段 | events | internal | Modify | N/A | cli | macOS UI | additive-only |

### 契约文档（按 Kind 拆分）

- None（本次使用代码内结构体 additive 变更，不新增独立 contracts 文档）

## 验收标准（Acceptance Criteria）

- Given backup 触发，When 处于 prepare，Then 进度条为 indeterminate 且文案为 Preparing。
- Given backup 进入 scan/upload/index，Then 四处进度条均显示双段确定性进度，且口径一致。
- Given 高去重目录，When backup 运行，Then 成功段持续增长并最终收敛到 100%。
- Given 旧 snapshot 缺少新增字段，When UI 渲染，Then 不崩溃并正确降级显示。

## 实现前置条件（Definition of Ready / Preconditions）

- 进度口径（成功段/扫描段）已冻结。
- prepare 并行语义与失败语义已冻结。
- 目标 UI 位点已冻结（MainWindow 3 处 + Popover 1 处）。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests:
  - core/daemon：新增字段透传与兼容。
  - cli：prepare 并行逻辑与 events 字段输出。
- Integration tests:
  - backup pipeline 在 prepare 并行下行为正确。

### Quality checks

- `cargo test`
- `cargo test -p televybackup-cli`
- `cargo test -p televybackupd`
- `scripts/macos/build-app.sh`

## 文档更新（Docs to Update）

- `README.md`: 说明 prepare 阶段与进度条语义。
- `docs/architecture.md`: 补充并行 prepare 与进度字段口径。

## 实现里程碑（Milestones / Delivery checklist）

- [x] M1: 完成 core/daemon/cli 进度字段扩展与兼容透传
- [x] M2: 完成 Prepare 并行（CLI + daemon）
- [x] M3: 完成主窗口与浮窗统一进度组件替换
- [x] M4: 完成测试、构建验证与文档同步

## 方案概述（Approach, high-level）

- 在 backup 前置阶段引入并行任务编排，并把 prepare 作为统一阶段对外暴露。
- 将 UI 进度逻辑组件化，剥离散落在多个视图中的重复判断与样式。
- 保持接口变更 additive，避免要求状态源与客户端同版本升级。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：本地快速统计在超大目录下耗时过长；需保证 metadata-only 且可取消。
- 开放问题：prepare 是否在 UI 上显示子任务完成计数（`1/2`）。
- 假设：双段颜色采用现有 Running 蓝系（成功段实蓝、扫描段浅蓝）。

## 变更记录（Change log）

- 2026-02-25: 新建规格并冻结并行 Prepare + 双段进度口径。
- 2026-02-25: 完成实现并通过 `cargo test`、`cargo test -p televybackup`、`cargo test -p televybackupd`、`scripts/macos/build-app.sh` 验证。

## 参考（References）

- 旧计划索引：`/Users/ivan/Projects/Ivan/televy-backup/docs/plan/README.md`
- 历史相关计划：
  - `/Users/ivan/Projects/Ivan/televy-backup/docs/plan/0010:status-popover-dashboard/PLAN.md`
  - `/Users/ivan/Projects/Ivan/televy-backup/docs/plan/0012:remote-first-index-sync/PLAN.md`
  - `/Users/ivan/Projects/Ivan/televy-backup/docs/plan/fwwqp:events-live-task-ui/PLAN.md`
