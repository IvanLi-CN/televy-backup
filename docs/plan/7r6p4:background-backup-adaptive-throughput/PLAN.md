# 后台备份吞吐自适应优化（稳定优先，尽量跑满带宽）（#7r6p4）

## 状态

- Status: 部分完成（4/5）
- Created: 2026-02-23
- Last: 2026-02-24
- Notes: PR #46（CI 已通过；30 分钟现场验收仍未达标，需继续）

## 背景 / 问题陈述

正式版 `Projects` 目标备份长期后台运行时出现“低吞吐 + 频繁超时 + 最终失败”的组合问题：

- 上传链路反复出现 `save_file_part/save_big_file_part/upload_stream` 超时。
- helper 与 core 之间存在“长调用期间无新响应”导致的 watchdog 误判风险。
- SQLite 在高负载写入阶段出现 `database is locked` / pool 相关失败，导致长时间运行后前功尽弃。
- 当前 endpoint 固定并发/固定节流配置对网络抖动适应性不足，难以稳定利用可用带宽。

本计划目标是在不改变备份范围与外部配置语义的前提下，使后台备份在复杂网络条件下更稳定，并尽可能持续利用带宽。

## 目标 / 非目标

### Goals

- 上传侧支持“内部自适应并发 + 自适应节流”，在稳定与吞吐之间自动平衡。
- 提升 MTProto helper 长调用期间的可观测性，减少 core 误判 helper 无响应。
- 提升 SQLite 锁竞争恢复能力，减少因短时锁冲突导致的整轮失败。
- 最终错误优先反映真实首个上传根因，不被后置 sqlite 错误覆盖。
- 验收满足以下任一条件：
  - 单次备份在 30 分钟内完成；
  - 30 分钟观察窗内，上传速率 `>= 1 MiB/s` 的累计时长 `>= 20` 分钟。

### Non-goals

- 不缩小备份范围（不新增目录排除策略）。
- 不新增对外配置字段（`config.toml` 语义保持兼容）。
- 不做整库“先删后建”索引重建（本轮先跑修复再观察）。
- 不修改 CLI/RPC 外部协议与接口。

## 范围（Scope）

### In scope

- `crates/core/src/backup.rs`
  - 上传调度引入内部自适应控制器（动态并发/动态 min delay）。
  - 输出 `upload.adaptive.tick` 结构化日志事件。
  - 关键写路径对 `locked/busy` 错误做有限重试。
  - 收敛阶段错误优先级修正（首个上传错误优先）。
- `crates/core/src/index_db.rs`
  - 打开数据库后设置 `PRAGMA busy_timeout=60000`。
- `crates/core/src/storage/telegram_mtproto.rs`
  - helper 响应 watchdog 阈值从 180s 调整到 600s。
- `crates/mtproto-helper/src/main.rs`
  - `send_message` 重试等待期间持续发 heartbeat progress。

### Out of scope

- WAL 模式切换与索引文件格式变更。
- 备份范围裁剪、目录排除、策略 UI 改造。
- 任何破坏性数据迁移。

## 验收标准（Acceptance Criteria）

- 功能层：
  - 后台备份在网络抖动下可自动调节上传并发与节流，`upload.adaptive.tick` 周期输出有效状态。
  - helper 不再因长调用“无新行输出”被 core 过早判死（在 600s watchdog 下仍可恢复推进）。
  - sqlite 锁竞争时优先进入重试恢复；不可恢复时错误上下文明确。
  - 若上传阶段已记录首个根因错误，最终返回该错误而非被后置 sqlite 错误覆盖。
- 性能层（现场验收，二选一通过）：
  - 30 分钟内完成；
  - 30 分钟窗口内 `>= 1 MiB/s` 累计时长 `>= 20` 分钟。

## Testing

- 自动化验证：
  - `cargo test -p televy_backup_core`
  - `cargo test -p televybackupd`
  - `cargo test -p televybackup`
  - `cargo test --manifest-path /Users/ivan/Projects/Ivan/televy-backup/crates/mtproto-helper/Cargo.toml`
- 现场验证：
  - 以 `Projects` 单目标运行 backup，并采样 30 分钟 status stream。
  - 统计是否 30 分钟内完成、以及 `>=1 MiB/s` 累计分钟数。

## Milestones

- [x] M1: core 上传侧自适应并发/节流控制器落地并接入主备份流程
- [x] M2: helper watchdog + send_message heartbeat 改造完成
- [x] M3: sqlite 锁竞争恢复（busy_timeout + 限定重试）落地
- [x] M4: 错误优先级修正 + 测试覆盖
- [ ] M5: 本地自动化验证 + 30 分钟现场验收 + PR 结果收敛

## 风险与开放问题

- 自适应策略在特定网络形态可能出现振荡，需要守住升降档节奏与上下限。
- 提高 watchdog 上限会延后“真死锁”暴露时间，需配合 heartbeat 与错误日志保障可观测性。
- sqlite 重试会增加尾延迟，需要限制重试次数与回退时间，避免无限拖延。

## 变更记录 / Change log

- 2026-02-23: 冻结目标、范围、验收与测试口径，进入实现阶段。
- 2026-02-23: 完成实现与本地自动化验证，创建 PR #46；现场采样约 897s，当前未满足 30 分钟验收口径。
- 2026-02-23: 完成 review 修复轮次并推送 `101ca61`，CI run #234 通过；验收仍待完整 30 分钟现场观察。
- 2026-02-23: 修正自适应上限/下限落地偏差（并发上限放开到内部 8、延迟下限允许降到 0ms）；补跑完整 30 分钟窗口（上传约 1.98 GiB，`>=1 MiB/s` 累计约 0.336 分钟，未在 30 分钟内完成），验收仍未通过。
- 2026-02-24: 追加 scan/上传流水线开销优化（扩大上传队列深度、pack 滞留超时刷新、scan 进度上报降频、`file_chunks` 按文件批量事务写入）；自动化测试通过。30 分钟窗口复测（`/tmp/7r6p4-perf-20260224-125752`）上传约 1.41 GiB，`>=1 MiB/s` 累计约 0.231 分钟，仍未满足验收。
- 2026-02-24: 按维护口径尝试一次“本地索引对齐远端 latest”（去掉 `--no-remote-index-sync`）时，运行在 preflight/index_sync 即失败：`config.invalid: bootstrap missing source_path: /Users/ivan/Projects`（`/tmp/7r6p4-perf-20260224-remoteidx-133022`），说明当前 bootstrap catalog 尚无该 target 的 latest 映射。
