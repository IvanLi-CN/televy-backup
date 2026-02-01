# Events（NDJSON）

> Kind: Event（internal）

## 新增 phase：`key_rotation`

- Change: Modify (additive)

当轮换任务运行时，进度事件应包含：

- `task.progress.phase = "key_rotation"`
- `task.progress.targetId`（如适用）
- `task.progress.step`（例如 `backup_full`, `upload_index`, `update_catalog`, `commit`）

（具体字段命名与现有 events 对齐，在实现阶段补齐。）

