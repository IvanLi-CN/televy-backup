# History

## Provenance

- Legacy source: `docs/plan/7r6p4:background-backup-adaptive-throughput/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 变更记录 / Change log

- 2026-02-23: 冻结目标、范围、验收与测试口径，进入实现阶段。
- 2026-02-23: 完成实现与本地自动化验证，创建 PR #46；现场采样约 897s，当前未满足 30 分钟验收口径。
- 2026-02-23: 完成 review 修复轮次并推送 `101ca61`，CI run #234 通过；验收仍待完整 30 分钟现场观察。
- 2026-02-23: 修正自适应上限/下限落地偏差（并发上限放开到内部 8、延迟下限允许降到 0ms）；补跑完整 30 分钟窗口（上传约 1.98 GiB，`>=1 MiB/s` 累计约 0.336 分钟，未在 30 分钟内完成），验收仍未通过。
- 2026-02-24: 追加 scan/上传流水线开销优化（扩大上传队列深度、pack 滞留超时刷新、scan 进度上报降频、`file_chunks` 按文件批量事务写入）；自动化测试通过。30 分钟窗口复测（`/tmp/7r6p4-perf-20260224-125752`）上传约 1.41 GiB，`>=1 MiB/s` 累计约 0.231 分钟，仍未满足验收。
- 2026-02-24: 按维护口径尝试一次“本地索引对齐远端 latest”（去掉 `--no-remote-index-sync`）时，运行在 preflight/index_sync 即失败：`config.invalid: bootstrap missing source_path: /Users/ivan/Projects`（`/tmp/7r6p4-perf-20260224-remoteidx-133022`），说明当前 bootstrap catalog 尚无该 target 的 latest 映射。
- 2026-02-24: 修复 index_sync 预检查在 bootstrap 缺失 target/latest 映射时的硬失败：降级为 `index_sync.skipped` 并继续使用本地索引执行备份，待本轮成功后再写回 remote latest。现场快检（`/tmp/7r6p4-perf-20260224-indexsync-skip2-135107/backup_run.jsonl`）已从 `index_sync` 顺利进入 `scan`，不再报 `bootstrap missing source_path`。
- 2026-02-24: 修复后补跑完整 30 分钟窗口（不带 `--no-remote-index-sync`，`/tmp/7r6p4-perf-20260224-full-135840/summary.json`）：上传约 0.97 GiB，`>=1 MiB/s` 累计约 0.102 分钟，未在窗口内完成，验收仍未通过。
- 2026-02-24: 增加 scan 热路径优化：预加载当前 provider 的 `chunk_objects` 到内存集合，替代逐 chunk SQLite 存在性查询，减少扫描期数据库往返；自动化测试通过。
- 2026-02-24: 继续完整 30 分钟窗口复测（不带 `--no-remote-index-sync`，`/tmp/7r6p4-perf-20260224-opt2-143423/summary.json`）：上传约 1.20 GiB，`>=1 MiB/s` 累计约 0.224 分钟，未在窗口内完成，验收仍未通过。
- 2026-02-24: 追加两项吞吐优化：`PACK_MAX_ENTRIES_PER_PACK` 从 32 提升到 1024（让 pack 更按字节填满，降低消息开销）；扫描阶段新增“同路径且元数据未变时复用 base snapshot 的 `file_chunks`”逻辑（避免重复分块与重复读盘）。自动化测试通过。
- 2026-02-24: 现场快检（`/tmp/7r6p4-perf-20260224-opt4-154855/summary.json`）观测 330s：扫描期 `bytesRead` 明显低于 `bytesDeduped`（已出现复用命中），但窗口内仍未出现有效上传速率，验收仍未通过；后续需继续优化大目录扫描/索引写入链路。
- 2026-02-24: 修复 PR CI 阻塞（`clippy::collapsible_if`，`crates/core/src/backup.rs` let-chain 合并），补跑自动化测试与 `cargo clippy --all-targets --all-features -- -D warnings` 通过；GitHub Actions CI run #242 通过。
- 2026-02-24: 补跑完整 30 分钟观察窗（`/tmp/7r6p4-perf-20260224-161434-opt6/summary.json`）：观测 `1806.24s`，上传 `1,337,256,265` bytes，`>=1 MiB/s` 累计 `13.18` 分钟，窗口结束时任务仍在进行，未满足“30 分钟内完成 / >=20 分钟达标速率”验收口径。
- 2026-02-24: 追加扫描热路径优化：base snapshot 的 `file_chunks` 复制改为批量事务提交（每 128 文件一批）以降低 SQLite 往返与提交开销；补跑自动化测试与 clippy 通过。
- 2026-02-24: 基于新优化补跑完整 30 分钟观察窗（`/tmp/7r6p4-perf-20260224-170237-opt7/summary.json`）：观测 `1803.90s`，上传 `1,782,790,489` bytes，`>=1 MiB/s` 累计 `18.37` 分钟（较上一轮 13.18 分钟提升），但仍未达到 `>=20` 分钟验收阈值。
- 2026-02-24: 试探性将上调阈值改为 `2 MiB/s` 后复测（`/tmp/7r6p4-perf-20260224-174019-opt8/summary.json`）：观测 `1806.01s`，上传 `1,731,059,576` bytes，`>=1 MiB/s` 累计 `17.30` 分钟，较 opt7 回退，未达标。
- 2026-02-24: 调整自适应上调策略（恢复 `1 MiB/s` 升档阈值、升档时 `min_delay` 每次下调 50ms、有 backlog 即可升档），并补跑完整 30 分钟观察窗（`/tmp/7r6p4-perf-20260224-181412-opt9/summary.json`）：观测 `1806.21s`，上传 `1,891,162,909` bytes，`>=1 MiB/s` 累计 `20.62` 分钟，达到验收口径（按累计时长通过）。
- 2026-02-24: PR #46 最新 head（`38dab4a`）CI run #245 通过，review 无阻塞评论；计划状态收敛为“已完成”。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/7r6p4:background-backup-adaptive-throughput/PLAN.md`.
