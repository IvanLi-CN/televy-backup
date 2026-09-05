# Events Contracts（status.snapshot）

本计划沿用 #0010 的 `StatusSnapshot`，并允许 additive-only 扩展。`targets[].activeTask` 是可选活动声明，包含 `kind=backup|restore|verify|sync` 和有序、去重的 `directions=up|down`；字段缺失时消费者必须保持旧快照兼容。它只在当前任务运行时出现，不能由 `lastRun` 或 `state=failed` 重建。
