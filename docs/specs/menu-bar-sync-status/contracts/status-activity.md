# Status Activity Contract

## Status snapshot

`StatusSnapshot.schemaVersion` 保持 `1`。以下字段是 additive-only；消费者必须在字段缺失时保持旧行为。

```ts
type ActivityDirection = "up" | "down";
type ActiveTask = {
  kind: "backup" | "restore" | "verify" | "sync";
  directions: ActivityDirection[];
};

type TargetState = {
  // Existing fields omitted.
  activeTask?: ActiveTask | null;
};
```

- `directions` 没有重复项，顺序稳定为 `up` 后 `down`。
- backup、restore、verify 的方向分别是 `["up"]`、`["down"]`、`[]`；sync 是 `["up", "down"]`。
- `activeTask` 只表示当前运行任务。它不携带终态，不得从 `lastRun` 回填。
- `backupQueue` 保持既有语义；排队成员没有 `activeTask`，但 macOS 菜单栏把它投影为备份活动。

## Control IPC

`status.taskStart` 指定任务类型；方向由 daemon 规范化生成：

```ts
type StatusTaskStartParams = {
  taskId: string;
  kind: "backup" | "restore" | "verify" | "sync";
  targetId: string;
  logging?: ResolvedLogging;
};
```

- daemon 为 backup、restore、verify 填入其规范方向；sync 使用其双向规范方向。
- 若目标已经有任何活动任务，daemon 返回 `ControlError { code: "target_busy", retryable: true, details: { targetId, activeKind } }`。CLI 必须保留该 code，而不是将其泛化为 `control.failed`。

`status.taskFinish` 参数新增可选 `errorCode`：

```ts
type StatusTaskFinishParams = {
  taskId: string;
  kind: "backup" | "restore" | "verify" | "sync";
  targetId: string;
  state: "succeeded" | "failed";
  errorCode?: string;
};
```

- 已接受任务的 finish 必须按 `taskId` 匹配；过期 finish 不得覆盖后续任务。
- daemon 在 `lastRun` 中保存外部任务的成功或失败终态和失败码，但清除 `activeTask`。

## macOS presentation

- `showMenuBarTransferRates` 是 `UserDefaults` Boolean，缺失值表示 `false`。
- 显示 `global.up.bytesPerSecond` / `global.down.bytesPerSecond` 时使用既有二进制单位格式。只要至少一个当前活动声明相应方向且值非负，即可显示该方向。
- 状态优先级为 live failure latch、bidirectional、backup、restore、verify、idle。failure latch 由 live transition 激活，10 秒后过期，绝不持久化。
