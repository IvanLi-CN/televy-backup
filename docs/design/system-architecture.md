# 系统架构设计：TelevyBackup

本文描述 TelevyBackup 的系统架构（组件边界、数据流、运行时职责与关键路径）。目标是让维护者能快速定位“数据从哪里来、由谁生成、在哪里落盘、UI 为什么会显示 stale/未连接”，并为后续演进（IPC、启动模型、可观测性）提供统一口径。

> 说明：仓库内同时存在 `docs/architecture.md`（偏实现说明、英文为主）。本文是面向设计与口径对齐的版本（中文为主），两者应保持一致。

## 1. 组件（Components）

### 1.1 macOS GUI（状态栏应用）

- 位置：`macos/TelevyBackupApp/`
- 技术：SwiftUI + AppKit（`NSStatusItem` + `NSPopover`）
- 责任：
  - Settings window：编辑 v2 Settings（targets/endpoints/schedule/recovery key 等）。
  - Popover Dashboard：展示全局 network + 多 target 状态；提供 Developer window 做排障。
  - 通过启动/管理本地 CLI/daemon（best-effort）来获取任务进度与状态快照。

### 1.2 CLI（televybackup）

- 位置：`crates/cli/`
- 责任：
  - 执行备份/恢复/校验等交互式命令。
  - 输出 NDJSON 事件流（stdout）供 GUI 低延迟消费。
  - 提供 `status get/stream`：读取 daemon 落盘的 `status.json` 并输出统一的 `StatusSnapshot`（NDJSON）。

### 1.3 Daemon（televybackupd）

- 位置：`crates/daemon/`
- 责任：
  - 定时任务（hourly/daily）：触发备份、执行保留策略（retention）。
  - 持续生成/更新状态快照：以原子写方式落盘 `status.json`，作为状态真源（source of truth）。
  - 推荐由用户级 LaunchAgent 管理（例如 Homebrew services），GUI 可对其做 best-effort kickstart。

### 1.4 Core（televy_backup_core）

- 位置：`crates/core/`
- 责任：
  - 备份管线：scan → CDC chunking → hash → framing(encrypt) → enqueue uploads → worker uploads → SQLite index。
  - 恢复/校验：使用远端 index manifest + chunk downloads。
  - 共享契约：`StatusSnapshot` schema、`status.json` 原子写工具等。

## 2. 关键数据与落点（Data Locations）

系统支持通过环境变量对齐 GUI/CLI/daemon 的工作目录：

- `TELEVYBACKUP_CONFIG_DIR`：配置目录（`config.toml` / `secrets.enc`）
- `TELEVYBACKUP_DATA_DIR`：数据目录（索引库、cache、status）
- `TELEVYBACKUP_LOG_DIR`：日志目录（默认 `TELEVYBACKUP_DATA_DIR/logs/`）

macOS 默认（未设置 env 时，GUI 侧）：

- `~/Library/Application Support/TelevyBackup/`
  - `status/status.json`
  - `logs/ui.log` 与每轮任务的 `sync-*.ndjson`

## 3. 状态快照（Popover/Developer Dashboard）

### 3.1 真源：daemon 落盘 `status.json`

- 文件路径：`$TELEVYBACKUP_DATA_DIR/status/status.json`
- 写入方式：临时文件写入 + `fsync` + `rename`（原子替换），避免 UI 读到半写入内容。
- 关键字段：
  - `generatedAt`：UI 用于 stale/disconnected 判定的时间基准。
  - `source.kind`：区分 `daemon`/`cli` 等来源（UI 用于解释“为什么看起来不是 live”）。

### 3.2 传输：CLI `status stream`（NDJSON）

- 命令：`televybackup --json status stream`
- 输出：NDJSON，每行一条 `status.snapshot`，UI 使用长生命周期进程持续读取（避免轮询与频繁拉起进程）。
- 兼容策略：当 `status.json` 缺失/不可读时，CLI 可输出基于 Settings 推导的“合成快照”（targets 列表仍可渲染，但标记为 stale）。

### 3.3 UI 语义：Live / Stale / Disconnected

Popover 右上角状态灯与 header 文案反映“快照新鲜度”：

- Live：来自 `daemon` 且 `now - generatedAt <= 2s`
- Stale：`now - generatedAt > 2s` 或来源不是 daemon（例如合成快照）
- Disconnected：`now - generatedAt > 10s` 或完全没有收到快照

如果用户看到“未连接/红色”，优先排查：

1) `televybackupd` 是否在跑（LaunchAgent / 进程）
2) `status.json` 是否在预期路径产生并更新
3) GUI 是否在读取同一个 `TELEVYBACKUP_*_DIR`（路径不一致会导致 UI 永远等不到快照）

## 4. 启动模型（Startup / Lifecycle）

### 4.1 推荐：LaunchAgent 管理 daemon

- 优点：符合 macOS 常规后台服务模型、生命周期稳定、避免多实例。
- GUI 行为：在 app launch 或打开 popover 时可 best-effort `launchctl kickstart`，减少“打开即未连接”的概率。

### 4.2 开发/本地：GUI 兜底直接拉起 daemon

- 当未安装 LaunchAgent 时，GUI 可从 app bundle（或 PATH）直接 spawn `televybackupd`。
- 目的：让本地构建即可直接体验“打开即有状态”，降低开发摩擦。

### 4.3 手动触发（Backup now）

- Popover Header 提供 `Backup now`（立即备份）按钮。
- 多 targets 策略（冻结）：点击后立即备份所有 `enabled=true` 的 targets（顺序/并发按后端既有策略）。
- 触发机制：GUI 写入控制文件 `$TELEVYBACKUP_DATA_DIR/control/backup-now`，daemon 侧检测并消费该触发（best-effort remove + run）。

## 5. 日志与可排查性（Observability）

- 任务日志（CLI/daemon）：以 NDJSON 落盘（每轮任务一份文件），避免混入 stdout（保证事件流可解析）。
- UI 日志：`ui.log`（best-effort 追加写），用于记录进程拉起、status stream 解析错误、stale 转换等。
- Developer window：用于把“快照来源/更新时间/原始字段”展示出来，尽量减少排障时对外部日志的依赖。
