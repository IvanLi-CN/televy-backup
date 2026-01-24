# File Formats Contracts（per-run sync logs）

> Kind: File format（internal）

## 1) 日志目录（Log directory）

每轮同步日志文件统一落在一个可打开的目录下。

目录优先级（从高到低）：

1. `$TELEVYBACKUP_LOG_DIR`
2. `$TELEVYBACKUP_DATA_DIR/logs/`
3. `~/Library/Application Support/TelevyBackup/logs/`（macOS 默认）

备注：

- macOS GUI 会在同一目录下追加写入 UI 日志文件 `ui.log`（用于排查 GUI/命令调用问题；best effort）。

## 2) 日志文件命名（Per-run）

每次同步任务（backup/restore/verify）生成一个新文件，不复用、不覆盖。

推荐命名（建议实现采用 UTC，避免夏令时/时区歧义）：

- `sync-<kind>-<started_at_utc>-<run_id>.ndjson`

字段：

- `<kind>`：`backup|restore|verify`
- `<started_at_utc>`：`YYYYMMDDTHHMMSSZ`
- `<run_id>`：`tsk_...` 或 `run_...`（稳定唯一；建议 UUID v4）

示例：

- `sync-backup-20260120T083012Z-tsk_8f3d....ndjson`

## 3) 文件内容格式（Format）

本计划固定为 **NDJSON（JSON Lines）**，便于自动分析与后续工具接入：

- 编码：UTF-8
- 分隔：每行一个 JSON object，以 `\\n` 分隔
- 必须包含的最小字段集（每行至少具备）：
  - `timestamp`（RFC3339；建议 UTC）
  - `level`
  - `target`
  - `fields`（object；至少包含 `message` 或等价字段）
- 推荐字段（用于排查；尽量覆盖）：
  - `kind`（`backup|restore|verify`，可来自文件名或记录字段）
  - `run_id`（可来自文件名或记录字段）
  - `spans`（list；包含 run 的 span 字段，用于关联）

说明：

- 允许额外字段（向后兼容），但不允许输出非 JSON 的混入行。

## 4) 落盘语义（Flush + fsync）

- 同步任务结束（成功/失败/取消）时必须 `flush + fsync`：
  - 保证该 run 的日志文件在任务返回后可读，并包含 `run.finish`（或等价的收尾记录）。
- 异常退出（panic/kill）：
  - 采用 best effort（尽量 flush）；`SIGKILL` 等场景无法保证。
