# 命令行（CLI）

> Kind: CLI（internal）

## `televybackup secrets rotate-master-key status`

- Scope: internal
- Change: New

```text
televybackup --json secrets rotate-master-key status
```

Output (JSON):

```json
{
  "state": "idle|staged|running|paused|cancelled|completed",
  "requestedAction": "none|pause|cancel",
  "pendingCatalogObjectIdByEndpoint": { "ep_a": "tgmtproto:v1:..." },
  "progress": {
    "targetsTotal": 3,
    "targetsDone": 1,
    "targets": [
      { "targetId": "t1", "state": "pending|running|done|failed" }
    ]
  },
  "nextAction": "none|resume|commit"
}
```

## `televybackup secrets rotate-master-key start`

- Scope: internal
- Change: New

```text
televybackup --json secrets rotate-master-key start
```

语义：

- `start` 为控制面操作：将轮换请求写入 rotation state 并让 daemon 异步开始执行（状态先进入 `staged`，随后进入 `running`）。
- GUI/CLI 通过 `status` 轮询或事件流展示进度；`pause/cancel` 可由另一个进程触发（见下文）。

Input (JSON on stdin):

```json
{
  "newGoldKey": "TBK1:...",
  "targetIds": ["..."],
  "confirm": { "ackRisks": true, "phrase": "ROTATE" }
}
```

Errors:

- `rotation.confirm_required`
- `rotation.invalid_state`
- `rotation.in_progress`: 已有轮换任务正在进行（retryable=true）

## `televybackup secrets rotate-master-key pause|resume|cancel`

- Scope: internal
- Change: New

```text
televybackup --json secrets rotate-master-key pause
televybackup --json secrets rotate-master-key resume
televybackup --json secrets rotate-master-key cancel
```

语义（跨进程控制，frozen）：

- `pause`：将 rotation state 的 `requestedAction` 置为 `pause`。daemon 在安全检查点尽快停下，并将 `state` 置为 `paused`（不清理 pending 信息）。
- `cancel`：将 `requestedAction` 置为 `cancel`。运行中的轮换任务在安全检查点尽快停下，并将 `state` 置为 `cancelled`，随后清理 `master_key.next` 与 rotation state（见 `contracts/config.md`）。
- `resume`：清除 `requestedAction`，并将状态推进为 `staged`（daemon 异步继续；随后进入 `running`）。

## `televybackup secrets rotate-master-key commit`

- Scope: internal
- Change: New

```text
televybackup --json secrets rotate-master-key commit
```

Input (JSON on stdin):

```json
{
  "confirm": { "ackRisks": true, "phrase": "ROTATE" }
}
```

Notes:

- commit 必须在所有目标完成后才允许；否则 `rotation.invalid_state`。

## Interaction: backup/restore/verify

当 rotation state 为 `staged|running|paused`（轮换进行中）时：

- `backup run` / `restore latest` / `verify` 必须拒绝启动并返回：
  - code: `rotation.in_progress`
  - retryable: true
  - message: 包含用户可操作提示（例如 “key rotation is in progress; pause/cancel to run backup/restore/verify”）
