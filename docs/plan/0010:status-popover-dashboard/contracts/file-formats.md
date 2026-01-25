# File formats Contracts

本文件定义（可选的）共享状态落盘格式，用于 daemon/CLI/UI 之间共享 `StatusSnapshot`。

## `status.json`

- Scope: internal
- Change: New
- Location:
  - 默认：`$TELEVYBACKUP_DATA_DIR/status/status.json`
  - 若 `TELEVYBACKUP_DATA_DIR` 未设置：按既有默认 data dir 规则（见 `docs/architecture.md`）

### Encoding

- UTF-8 JSON
- 必须为单个 `StatusSnapshot` 对象（同 `status.snapshot` schema）

### Write semantics

- 原子更新（write temp + rename）以避免 UI 读到部分内容。
- `generatedAt` 与 `source` 字段必须真实反映生成者与时间。
- `global.*Total` / `targets[].upTotal` / `global.uiUptimeSeconds` 属于 UI/stream session 口径：当写入方为 daemon 时通常应为 `null`/缺省（由 CLI/UI 侧在消费时累积/补齐）。

### Read semantics

- 读失败（文件不存在/JSON 无法解析）视为 `status.unavailable`（由 `status get/stream` 转换为 error 或空快照，取决于实现策略）。
