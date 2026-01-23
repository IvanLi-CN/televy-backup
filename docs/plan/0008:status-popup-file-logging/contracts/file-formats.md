# File Formats Contracts（UI logs）

> Kind: File format（external）

## 1) UI 日志文件（`ui.log`）

- 范围（Scope）: external
- 变更（Change）: Modify
- 编码（Encoding）: UTF-8（text）

### Path（位置）

最终口径（冻结）：

- UI 日志文件固定放入“后端 per-run logs 目录”下，便于统一排查与统一打开：
  1. 若设置 `TELEVYBACKUP_LOG_DIR`：`$TELEVYBACKUP_LOG_DIR/ui.log`
  2. 否则：`$TELEVYBACKUP_DATA_DIR/logs/ui.log`
  3. macOS 默认：`~/Library/Application Support/TelevyBackup/logs/ui.log`

备注：

- 若 `TELEVYBACKUP_LOG_DIR` 指向不可写位置：UI 文件写入需 best effort，不得导致 UI 崩溃；并应给出可见提示（toast/提示文案的具体口径由 UI 契约约束）。

### Compatibility / migration（兼容性与迁移）

- 旧位置不做迁移；新日志写入新位置；文档与 UI 入口统一指向新位置。

### Format（行格式）

- 追加写（append-only）
- 每条记录一行，以 `\n` 分隔
- 每行结构：
  - `<timestamp> <message>`
  - `timestamp`：ISO-8601（UTC 推荐；至少包含时区信息）

示例：

- `2026-01-22T13:06:12Z UI started`
- `2026-01-22T13:06:19Z ERROR: settings get failed: exit=1 reason=...`

### Redaction（脱敏）

- 若日志行包含 `api.telegram.org` 的 URL 片段，需要将该片段替换为占位符（例如 `[redacted_url]`），避免泄露 secret-bearing URL（如 token）。

### Failure semantics（失败语义）

- 文件写入失败：best effort；不得导致 UI 崩溃，不得递归触发日志写入。
