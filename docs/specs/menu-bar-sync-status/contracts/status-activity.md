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
  processId?: number;
  logging?: ResolvedLogging;
};
```

- daemon 为 backup、restore、verify 填入其规范方向；sync 使用其双向规范方向。
- 若目标已经有任何活动任务，daemon 返回 `ControlError { code: "target_busy", retryable: true, details: { targetId, activeKind } }`。CLI 必须保留该 code，而不是将其泛化为 `control.failed`。
- 新版 CLI 发送自身 `processId`；daemon 发现该进程退出时以 `task.reporter_lost` 结束外部任务并释放目标。旧客户端缺少该字段时，daemon 在保守的报告超时后执行同样的清理。

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

- 已接受任务的 finish 必须按 `taskId` 与 `kind` 匹配；过期 finish 不得覆盖后续任务。
- 成功响应仅表示两种结果之一：daemon 刚刚应用匹配终态，或 daemon 在当前运行期保留了完全相同的终态并确认该请求是幂等重放。响应结果含 `acknowledged=true`，并以 `replayed` 区分两者。不同状态、失败码或任务种类的重放必须被拒绝。
- status 状态锁不可用时返回 `control.unavailable`；未知目标返回 `target_not_found`；没有匹配 live 所有权或终态重放记录时返回 `task_not_owned`。daemon 重启后不保留重放记录，因此不能确认旧任务的 finish。
- daemon 在 `lastRun` 中保存外部任务的成功或失败终态和失败码，但清除 `activeTask`。CLI 只有收到 `acknowledged=true` 才把成功的数据面命令报告为成功；失败的数据面命令在终态上报无法确认时仍保留其原始错误。
- 通用 `restore run` 和 `verify run` 必须通过 snapshot 的 `source_path` 与 endpoint 解析唯一配置目标后才能发送 `status.taskStart`。缺少 source、无匹配或多个匹配均为失败关闭，不能执行数据面。

## macOS presentation

- `showMenuBarTransferRates` 是 `UserDefaults` Boolean，缺失值表示 `false`。
- 显示 `global.up.bytesPerSecond` / `global.down.bytesPerSecond` 时，每个当前活动声明方向使用一个右对齐、等宽的四字符速率槽；箭头与 `/s` 后缀不计入槽宽。单位按二进制量级选择并压缩为单字符 `B/K/M/G/T/P/E`，小于 `10` 的非字节量级保留一位小数，其余显示整数。读数在当前量级下将超过四字符时立即进位。
- 声明方向的零速显示 `0B`；速率缺失或负值显示 `----`，不得移除该方向段。只有连接不再 fresh 时才隐藏全部速率。
- 状态优先级为 live failure latch、bidirectional、backup、restore、verify、idle。failure latch 由 live transition 激活，10 秒后过期，绝不持久化。相同 live 任务的本地事件和 daemon 终态共享一个期限；不同任务拥有独立期限。
