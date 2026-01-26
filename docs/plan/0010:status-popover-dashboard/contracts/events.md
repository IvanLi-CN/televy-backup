# Events Contracts（NDJSON）

本文件定义 `status stream` 输出的 NDJSON 事件契约。

## `status.snapshot`

- Scope: internal
- Change: New
- Producer: `televybackup status stream`（CLI），（可选）daemon 写入状态源后由 CLI 转发
- Consumers: macOS UI（Popover Overview / Dev）
- Delivery semantics: best-effort；UI 以 `generatedAt` 做 stale 判定；事件允许丢失（因为每条都是完整快照）

### Semantics（职责与口径）

- `source.kind`/`source.detail` 表示**上游状态来源**（用于排障定位“这条快照从哪里来”），并不等价于“NDJSON transport 的输出进程”。
  - 本计划默认：daemon 写入 `status.json`，`status stream` 读取并转发，因此 `source.kind` 通常为 `daemon`（即使 NDJSON 是由 CLI 输出）。
  - `cli`/`file` 仅用于“无需 daemon 的快照”（例如纯 CLI 直算、或读取离线文件/fixture）等场景。
- `global.*Total` 与 `targets[].upTotal` 表示“**自 UI/stream 启动以来**累计值”（非持久化，重启清零），由 UI/CLI 侧在渲染/转发时负责累积；当未知/未实现时，输出 `{ "bytes": null }`（不要省略字段）。
- `global.uiUptimeSeconds` 表示 UI/stream 的 session uptime（秒），由 UI/CLI 侧提供；当未知/未实现时缺省或 `null`。

### Payload schema

```ts
type UnixMs = number; // epoch milliseconds

type Rate = {
  bytesPerSecond: number | null; // null means unknown/not available
};

type Counter = {
  bytes: number | null; // null means unknown/not available
};

type Progress = {
  phase: string; // e.g. "scan" | "pack" | "upload" | "verify" | "running"
  filesTotal?: number | null;
  filesDone?: number | null;
  chunksTotal?: number | null;
  chunksDone?: number | null;
  bytesRead?: number | null;
  bytesUploaded?: number | null;
  bytesDeduped?: number | null;
};

type TargetRunSummary = {
  finishedAt?: string | null; // ISO8601
  durationSeconds?: number | null;
  status?: "succeeded" | "failed" | null;
  errorCode?: string | null; // short machine code
  filesIndexed?: number | null; // count of indexed files in that run (when known)
  bytesUploaded?: number | null;
  bytesDeduped?: number | null;
};

type TargetState = {
  targetId: string;
  label?: string | null;
  sourcePath: string;
  endpointId: string;
  enabled: boolean;

  state: "idle" | "running" | "failed" | "stale";

  runningSince?: UnixMs | null;

  // Realtime rate (per target; business-level bytesUploaded rate)
  up: Rate;

  // Session totals since UI/stream start (per target; bytes may be null)
  upTotal: Counter;

  progress?: Progress | null; // present when running
  lastRun?: TargetRunSummary | null; // present when known
};

type StatusSnapshot = {
  type: "status.snapshot";
  schemaVersion: 1;
  generatedAt: UnixMs;
  source: {
    kind: "daemon" | "cli" | "file";
    detail?: string | null; // e.g. pid/path
  };

  global: {
    // Business-level semantics (frozen in PLAN.md):
    // - up = rate/total of bytesUploaded
    // - down = rate/total of bytesDownloaded (may be null/not applicable for backup)
    up: Rate;
    down: Rate;
    upTotal: Counter;
    downTotal: Counter;
    uiUptimeSeconds?: number | null;
  };

  targets: TargetState[];
};
```

### Validation

- `schemaVersion` 必须为整数且目前固定 `1`。
- `generatedAt` 必须单调不减（同一进程/同一 source）。
- `targets[].targetId/sourcePath/endpointId` 必须非空。

### Example

（示例值仅用于形状说明）

```json
{
  "type": "status.snapshot",
  "schemaVersion": 1,
  "generatedAt": 1769212800123,
  "source": { "kind": "daemon", "detail": "televybackupd (status.json)" },
  "global": {
    "up": { "bytesPerSecond": 3355443 },
    "down": { "bytesPerSecond": 8192 },
    "upTotal": { "bytes": 4294967296 },
    "downTotal": { "bytes": 268435456 },
    "uiUptimeSeconds": 732.4
  },
  "targets": [
    {
      "targetId": "t_abcd1234",
      "label": "Photos",
      "sourcePath": "/Volumes/SSD/Photos",
      "endpointId": "ep_default",
      "enabled": true,
      "state": "running",
      "up": { "bytesPerSecond": 3355443 },
      "upTotal": { "bytes": 268435456 },
      "progress": {
        "phase": "upload",
        "filesTotal": 1200,
        "filesDone": 640,
        "chunksTotal": 4020,
        "chunksDone": 2100,
        "bytesRead": 987654321,
        "bytesUploaded": 268435456,
        "bytesDeduped": 2147483648
      },
      "lastRun": null
    }
  ]
}
```

### Compatibility rules

- Additive changes only：允许新增字段（UI 必须容错）；删除/重命名字段需 bump `schemaVersion`。
