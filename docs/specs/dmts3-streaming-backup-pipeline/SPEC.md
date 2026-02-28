# backup 主流水线并行化（scan+upload）与进度语义修复（#dmts3）

## 状态

- Status: 部分完成（4/5）
- Created: 2026-02-26
- Last: 2026-02-28

## 背景 / 问题陈述

- 当前 `Prepare` 已并行，但 backup 主体仍是先 `scan` 后 `upload`，导致扫描阶段长时间仅有扫描进度，上传指标长期为 0。
- 用户观感出现冲突：`Scanned` 接近 100% 时，实际上传仍未启动或未显著推进，无法反映真实瓶颈。
- 需求已明确：主循环应支持“扫描产出即上传”，并通过稳定口径展示扫描、已备份、上传中、需上传等进度。

## 目标 / 非目标

### Goals

- 将 backup 主循环改为并行流水线：
  - Job A：遍历/切块/hash/去重/索引写入。
  - Job B：消费待上传块并并发上传（保留现有限流与并发控制）。
- 扫描阶段允许上传同步进行，不再强制 “scan 完成后才 upload”。
- 保持进度字段 additive 兼容，并修复 UI 语义冲突：
  - 仅 `prepare` 为 indeterminate。
  - `scan/upload/index` 均为 determinate，且分段含义单调不反超。
- 保持 checkpoint/恢复能力（上传过程持续落盘）。

### Non-goals

- 不重构 restore/verify。
- 不更改远端存储协议。
- 不引入新的外部依赖或后台服务。

## 范围（Scope）

### In scope

- `crates/core/src/backup.rs` 主循环改造为 scan+upload 并行。
- 进度阶段文本与口径修正（含 `scan_upload` 显示兼容）。
- 单元测试补齐：并行启动、阶段推进、字段单调性。

### Out of scope

- 完整重做 UI 视觉风格。
- 跨进程协议版本升级（仅 additive 字段或新 phase 字符串）。

### 补充范围（2026-02-27）

- 备份前 retention 清理的性能优化与可观测性增强：
  - 快照删除改为批处理（snapshot batch + file batch）以缩短单次长事务时间。
  - 增加 retention 批次完成日志，便于 UI 卡在 prepare 时定位进度。
  - 通过 migration 调整索引，降低 retention 热路径扫描成本。

### 补充范围（2026-02-28）

- index 上传内存足迹治理：
  - `upload_index` 不再整库 `fs::read + encode_all`，改为“流式压缩到临时文件 + 分片读取上传”。
  - 目标：避免大索引（数 GB）在 daemon 内形成单次巨型堆分配并长期保留高 footprint。

## 需求（Requirements）

### MUST

- 一旦扫描发现首个缺失 chunk，必须允许上传 worker 开始工作。
- `scan` 未完成时允许 `bytes_uploaded*` 正向增长。
- `source_bytes_need_upload_total` 必须单调不减，且最终反映本轮 source payload 需上传总量。
- 失败时状态不得回退为 prepare 式滚动条。
- index 阶段上传必须是有界内存模型；不得依赖“整库读入内存”策略。

### SHOULD

- 当扫描和上传并行时，阶段文案显示 `Scanning + Uploading`（或等价语义）。
- 进度分段满足单调：`NeedUploadConfirmed <= UploadingCurrent <= BackedUp <= Scanned`。

### COULD

- 在 UI 上展示“上传队列积压（pending bytes/jobs）”用于定位瓶颈。

## 功能与行为规格（Functional/Behavior Spec）

### Core flow

1. `Prepare`（并行）：`index_sync + local_quick_stats`。
2. `scan` 启动后，Job A 每发现新缺失 chunk 即进入加密打包并入队，Job B 可立即上传。
3. `scan` 完成后，关闭上传入队通道；等待队列消费完成。
4. 执行 `index` 上传与收尾。

### Error/Cancel

- 上传失败：保留当前失败语义（首错优先），并停止后续流水线。
- 扫描取消：立即传播 cancel，上传 worker 尽快退出。
- 文件瞬态变化：沿用现有“剪枝有问题文件并继续”策略，不因单文件变动中断整轮。

## 验收标准（Acceptance Criteria）

- Given 大目录备份，When `scan` 进行中，Then `bytes_uploaded` 在 `scan` 阶段即可增长（不再长期为 0）。
- Given 进入并行阶段，Then UI 阶段文案与分段进度不冲突、不反超。
- Given 运行失败，Then 进度条保持 determinate 结果态，不回退为 prepare 风格滚动条。
- Given 旧客户端字段缺失，Then UI 仍能降级渲染，不崩溃。

## 测试与验证（Testing）

- `cargo test -p televy-backup-core`
- `cargo test -p televybackup-cli`
- `cargo test -p televybackupd`
- `scripts/macos/swift-unit-tests.sh`
- `cargo test -p televy_backup_core --test pack_uploads`

## 里程碑（Milestones）

- [x] M1: core 主循环 scan+upload 并行化
- [x] M2: 阶段与进度字段语义对齐
- [x] M3: 单元测试覆盖并通过
- [x] M4: retention 清理性能优化（批处理 + 索引调整 + 回归测试）
- [ ] M5: macOS UI 验证与截图确认

## 变更记录（Change log）

- 2026-02-26: 新建规格，冻结并行主流水线与进度语义修复目标。
- 2026-02-26: 主循环改为“扫描发现缺失即入队上传”，并补齐 core/cli/daemon/swift 单测验证。
- 2026-02-27: retention preflight 改为批处理删除并加入批次日志，新增 migration `0003_retention_perf.sql` 调整索引，`televy_backup_core` 回归测试通过。
- 2026-02-27: 为 index part + manifest 上传补齐重试/退避（与数据块上传一致），避免单次 MTProto 45s 超时直接导致整轮失败；补充 `pack_uploads` 重试用例。
- 2026-02-28: `upload_index` 改为流式压缩上传（临时文件分片读取），移除整库读入内存路径；新增大索引多分片上传测试 `large_index_db_uploads_multiple_index_parts`。
