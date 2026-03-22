# 规格（Spec）总览

本目录用于管理工作项的**规格与追踪**：记录范围、验收标准、任务清单与状态，作为交付依据；实现与验证应以对应 `SPEC.md` 为准。

> Legacy compatibility: historical specs remain under `docs/plan/**/PLAN.md`. New or migrated entries should be created under `docs/specs/**/SPEC.md`.

## 快速新增一个规格

1. 生成一个新的规格 `ID`（推荐 5 个字符的 nanoId 风格，降低并行建规格时的冲突概率）。
2. 新建目录：`docs/specs/<id>-<title>/`（`<title>` 用简短 slug，建议 kebab-case）。
3. 在该目录下创建 `SPEC.md`。
4. 在下方 Index 表新增一行，并把 `Status` 设为 `待设计` 或 `待实现`，并填入 `Last`。

## 状态（Status）说明

仅允许使用以下状态值：

- `待设计`
- `待实现`
- `跳过`
- `部分完成（x/y）`
- `已完成`
- `作废`
- `重新设计（#<id>）`

## Legacy plan index

历史计划索引仍在：`docs/plan/README.md`。

## Index（固定表格）

| ID   | Title | Status | Spec | Last | Notes |
|-----:|-------|--------|------|------|-------|
| n2kbu | PR + Label 发布能力 | 已完成 | `n2kbu-pr-label-release/SPEC.md` | 2026-03-08 | 已补 frozen intent、required checks 声明、bootstrap waiver；GitHub ruleset 已于 2026-03-08 对齐 |
| 3rnws | Main Window：Targets 菜单补齐 “Backup now” | 已完成 | `3rnws-main-window-target-backup-menu/SPEC.md` | 2026-03-05 | PR #51；CI run #273 通过；review-loop 无阻塞问题 |
| 2e73n | Popover Targets 高度实时自适应与误滚动修复 | 已完成 | `2e73n-popover-targets-live-height/SPEC.md` | 2026-02-25 | PR #48 已创建并更新；CI run #251 通过；review-loop 无阻塞问题 |
| dmts3 | backup 主流水线并行化（scan+upload）与进度语义修复 | 部分完成（4/5） | `dmts3-streaming-backup-pipeline/SPEC.md` | 2026-02-28 | 主循环并行 + retention 优化已落地；新增 index 流式压缩上传以压低 daemon 内存 footprint，待完成 UI 真机截图验收 |
| z324m | 统一进度条规范（含 Prepare 并行）与四处 UI 对齐 | 已完成 | `z324m-unified-backup-progress-prepare/SPEC.md` | 2026-02-28 | 已同步为单条多层进度规范（NeedUploadConfirmed/UploadingCurrent/BackedUp/Scanned）并保留 Need Upload(Disc./Final) 口径 |
| hqjd2 | MTProto 备份传输提速（更大分片 + FloodWait 处理 + 可调节流） | 已完成 | `hqjd2-mtproto-transfer-speed/SPEC.md` | 2026-03-02 | PR #49（open）；follow-up：helper pool 并行 uploads + helper session 隔离 |
| dyu56 | 索引分级：Remote Index 仅保留每个 Source 最新文件映射 | 部分完成（3/4） | `dyu56-index-tiered-filemaps/SPEC.md` | 2026-03-02 | 已实现 remote export + 本地自动 compact + 单测；待真机验证 index 上传耗时与体积收益 |
| t764g | 端点索引二级拆分：Endpoint DB（一级）+ Snapshot Filemap DB（二级）+ 严格远端门禁（Fail Fast） | 已完成 | `t764g-endpoint-two-level-index/SPEC.md` | 2026-03-02 | 已落地两级索引（endpoint DB + per-snapshot filemap DB）、base filemap 预取与严格远端门禁；restore/verify 支持 ATTACH 两级 DB 并兼容旧格式 |
| 3z7rj | Endpoint 去重索引增量化：Remote Delta + 本地物化库 + 周期性 Compaction | 已完成 | `3z7rj-endpoint-dedupe-delta-index/SPEC.md` | 2026-03-02 | Remote dedupe 由 Base+Delta+Catalog 组成；endpoint meta DB 不再上传 chunks/chunk_objects；restore/verify 优先使用 dedupe DB |
| g7gt3 | 支持 `.televyignore` 的文件/目录忽略能力 | 已完成 | `g7gt3-televyignore-target-ignore/SPEC.md` | 2026-03-19 | core 扫描/quick stats 已支持 `.televyignore`；run.finish 输出 ignore 汇总字段；macOS 主界面显示 ignore 状态；正式版补齐 bundled daemon/helper 启动路径稳定性修复 |
| cac6x | MTProto 空闲 Helper 退出治理 | 已完成 | `cac6x-mtproto-helper-idle-shutdown/SPEC.md` | 2026-03-22 | internal lifecycle 修复：graceful shutdown + kill fallback，解决 idle orphan helper 高 CPU；macOS 历史页改为磁盘回填 + 大日志头尾索引 |
