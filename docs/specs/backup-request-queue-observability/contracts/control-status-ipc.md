# Control 与 Status IPC 合同

## `backup.enqueue`

请求为现有 control envelope 的 `method: "backup.enqueue"`。`params` 必须恰好指定以下一种 scope：

```json
{ "scope": "allEnabled" }
```

```json
{ "scope": "targets", "targetIds": ["target-a", "target-b"] }
```

`allEnabled` 在接收时按当前 settings 中的 enabled targets 冻结；`targets` 按当前 settings 顺序去重，且允许 disabled target。空 `targetIds`、未知 target、无 enabled target、不可用 vault/master key 或全局 Telegram API 凭据必须返回既有 control error envelope，并以稳定 `code` 区分。

成功 result：

```json
{
  "batchId": "opaque-id",
  "disposition": "accepted",
  "targetIds": ["target-a", "target-b"]
}
```

`disposition` 为 `accepted` 表示创建新的活动或后续批次；`coalesced` 表示请求合并到已有批次。所有响应 target ids 都是最终、去重且按配置排序的集合。

## `backup.stop`

请求为现有 control envelope 的 `method: "backup.stop"`，`params` 必须为空对象：

```json
{}
```

成功 result：

```json
{
  "cancellationRequested": true,
  "clearedTargetIds": ["target-a", "target-b"]
}
```

- `cancellationRequested` 表示 daemon 已向当前备份任务发出取消请求。
- `clearedTargetIds` 是已从活动/后续手动批次移除的目标集合。
- 该调用不停止 daemon，也不修改定时备份设置；取消完成后，运行目标回到 `idle` 并以 `lastRun.status="cancelled"` 记录结果。

## `status.snapshot.targets[].backupQueue`

`backupQueue` 为 additive optional 字段：

```json
{
  "activeBatchId": "opaque-id-or-null",
  "pendingBatchId": "opaque-id-or-null"
}
```

- 非运行成员的任一非空字段投影为 `Queued`。
- 正在运行成员沿用 `state: "running"` 和 `progress.phase`；`pendingBatchId` 非空时附加 `Next queued`。
- `state` 的历史允许值不因本合同扩展。缺失 `backupQueue` 的旧 snapshot 保持旧客户端行为。
- 只要任一 batch 存在，status IPC 视为 active，以运行态刷新频率推送。
