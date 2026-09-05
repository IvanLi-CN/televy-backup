# DB Contracts（SQLite index）

> Kind: DB（internal）
>
> Engine: SQLite
>
> Migration: 使用 `schema_migrations` 版本表 + 逐步迁移（策略为 forward-only）

本计划的 DB 变更目标：让索引能够表达“chunk 存在于 pack 中的某个 offset/len”，而不是每个 chunk 对应一个独立 Telegram `file_id`。

## 1) Chunk 远端引用编码（`chunk_objects.object_id`）

- 范围（Scope）: internal
- 变更（Change）: Modify
- 影响表（Affected tables）: `chunk_objects`

### Change（旧 → 新）

旧（#0001，MVP）：

- `object_id = <telegram_file_id>`（一个 chunk 对应一个 Telegram document）

新（#0002，增量）：

`object_id` 允许两种形状（字符串编码，便于兼容与迁移）：

1. 独立对象（保持兼容）：
   - `tgfile:<file_id>`
2. pack 内切片（新增）：
   - `tgpack:<file_id>@<offset>+<len>`

字段语义：

- `<file_id>`：Telegram Bot API `file_id`
- `<offset>`：十进制字节偏移（从 pack 文件起始算起）
- `<len>`：十进制字节长度（对应 pack 内该 chunk 的“加密 blob”长度）

约束/兼容性：

- 迁移期内允许旧数据保持裸 `file_id`（无前缀）；读取端需同时兼容 `file_id` / `tgfile:` / `tgpack:` 三种形状。
- 新写入建议统一使用带前缀的新形状，避免歧义。

### Migration notes（迁移说明）

- 向后兼容窗口（Backward compatibility window）: 至少 1 个版本窗口同时支持旧 `file_id` 与新前缀编码
- 发布/上线步骤（Rollout steps）:
  1) 先上线读取兼容（能够解析三种 `object_id`）
  2) 再开启 pack 写入（写入 `tgpack:`）
  3) 最后按需做数据整理（可选：把裸 `file_id` 规范化为 `tgfile:`）
- 回滚策略（Rollback strategy）:
  - 关闭 pack 写入，继续写入 `tgfile:`；读取端保持兼容即可
- 回填/数据迁移（Backfill / data migration）:
  - 可选：将旧裸 `file_id` 更新为 `tgfile:<file_id>`（不影响语义）
