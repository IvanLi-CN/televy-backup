# 修复上传速度显示不正确（MTProto progress + status stream）（#fh5ac）

## 状态

- Status: 已完成
- Created: 2026-02-11
- Last: 2026-02-11
- Notes: PR #38；待主人验收（建议对齐 Activity Monitor 中 `televybackup-mtproto-helper` 的发送速率）

## 背景 / 问题陈述

- 现象：软件内显示的上传速度（popover 全局 ↑ / target 行 ↑）与 macOS 监控看到的真实网卡吞吐存在明显差异，且缺乏正相关性。
- 初步定位：
  - `televybackup-mtproto-helper` 的 `bytesUploaded` 上报在部分路径上可能“提前累计”（并非代表已实际完成上传）。
  - `televybackup --json status stream` 会对 daemon snapshot 的 `bytesPerSecond` 做二次计算并覆盖，导致速率呈现脉冲式跳变。
  - daemon 侧速率采样窗口会被“bytesUploaded 未变化的 scan/progress 更新”推进，导致 dt 被压缩，从而出现速率乱跳（看起来与真实网络脱钩）。

## 目标 / 非目标

### Goals

- 上传速度与 macOS 监控（优先以 `televybackup-mtproto-helper` 进程的发送速率为基准）呈明显正相关，并保持同量级。
- `bytesUploaded` 语义收敛为“已成功上传（remote 侧已确认接收）的 payload bytes”，避免提前跑进度。
- `status stream` 输出的 `bytesPerSecond` 不再出现与真实网络脱钩的脉冲/乱跳。

### Non-goals

- 不追求与网卡 wire-level 吞吐严格一致（协议开销、系统开销允许存在）。
- 不变更 StatusSnapshot schema（字段不改名/不删字段；仅修复计算/赋值逻辑）。
- 不引入新的遥测系统或持久化状态（保持现有本地 status/IPC 体系）。

## 范围（Scope）

### In scope

- MTProto helper（`crates/mtproto-helper/`）：
  - small file：进度上报改为在 `SaveFilePart` 成功后再累计与上报。
  - big file：并发 worker 仅在 `SaveBigFilePart` 成功后累计 uploaded；移除失败回滚逻辑（避免非单调）。
- CLI status stream（`crates/cli/`）：
  - 保留 “session totals” (`upTotal`) 的 enrich 行为；
  - `bytesPerSecond`：优先保留 daemon 计算值（并在 stale 时隐藏），不再无条件覆盖。
- daemon status（`crates/daemon/`）：
  - `bytesPerSecond` 的采样窗口仅在 `bytesUploaded` 前进时推进（避免 scan/progress 更新把 dt 压缩成脉冲）。
- 回归测试：为 enricher 行为补齐最小单元测试覆盖。

### Out of scope

- 调整 daemon status writer cadence / IPC 协议。
- UI 展示布局调整（仅修复数据源语义与计算）。

## 需求（Requirements）

### MUST

- `bytesUploaded` 单调递增（同一次上传过程中不回退）。
- `status stream` 不覆盖 snapshot 已提供的 `target.up.bytesPerSecond`（在 running 且非 stale 时）。
- `global.up.bytesPerSecond` 与 targets 的速率表现一致（同一套 gating；避免“全局有值但所有 target 为 —”）。
- 至少补齐 1 组单元测试（覆盖“不覆盖 daemon rate”与 stale gating）。

## 验收标准（Acceptance Criteria）

- Given 触发一次持续上传（>=20s），When 在 macOS 监控中观察 `televybackup-mtproto-helper` 的发送速率，
  Then UI 显示的 ↑ 上传速度随其升降变化，且不再出现“网卡未动但 UI 先飙高/乱跳”的现象。

- Given status snapshot 超过 2s 未更新（stale），Then UI 不展示实时上传速率（显示为 `—` 或明显 stale 提示）。

## 质量门槛（Quality Gates）

- 通过至少一次本地自动化验证（以 `cargo test` 为主，范围可按改动最小化）。

## 里程碑（Milestones）

- [x] M1: 修复 MTProto helper progress 上报语义（仅成功后累计）
- [x] M2: CLI status stream 不覆盖 daemon 速率（保留 session totals）
- [x] M3: 补齐最小单测 + 本地验证通过
- [x] M4: daemon 侧速率采样仅随 `bytesUploaded` 前进（避免 scan/progress 干扰）

## 风险与开放问题（Risks / Open Questions）

- 速率口径为 payload bytes/s，可能与网卡统计存在固定比例偏差（协议/系统开销）；但应保持正相关且同量级。
- big file 并发上传时，成功累计的粒度依赖 part 完成节奏；短时间内可能更“阶梯”，但应总体更可信。

## 变更记录 / Change log

- 2026-02-11：修复 MTProto helper progress 语义（仅成功后累计）+ status stream 不覆盖 daemon 速率；补齐单元测试与本地验证。
- 2026-02-11：daemon 侧修复速率采样窗口推进逻辑（仅在 `bytesUploaded` 前进时更新时间基准），避免 scan/progress 造成的速率脉冲。
- 2026-02-11：daemon 侧在主循环消费手动备份触发文件（`control/backup-now`），修复点击 Start 后不启动（trigger 未被消费）。
