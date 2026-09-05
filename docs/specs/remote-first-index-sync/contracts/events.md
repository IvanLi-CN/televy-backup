# Events Contract：新增 `index_sync` phase（#0012）

## 背景

现有备份任务会按 phase 输出进度（例如 `scan/upload/index`）。本计划在 `scan` 前新增一个前置 phase，用于索引对齐。

## 约定

- phase 名称：`index_sync`
- 兼容性：additive-only（旧 UI 遇到未知 phase 应保持可容错）。

## 事件

### `phase.start`

- `phase = "index_sync"`

### `phase.finish`

- `phase = "index_sync"`
- 可选字段（若已有惯例则复用）：
  - `duration_ms`
  - `bytes_downloaded`（如果本次发生了远端索引下载）
  - `index_source`（例如 `skipped|downloaded`，仅用于可观测性）
