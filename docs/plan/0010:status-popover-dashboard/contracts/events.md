# Events Contracts（NDJSON）

本文件定义 `status stream` 输出的 NDJSON 事件契约。

## `status.snapshot`

- Scope: internal
- Change: New
- Producer: `televybackup status stream`（CLI），（可选）daemon 写入状态源后由 CLI 转发
- Consumers: macOS UI（Popover Overview / Dev）
- Delivery semantics: best-effort；UI 以 `generatedAt` 做 stale 判定；事件允许丢失（因为每条都是完整快照）

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
};

type TargetState = {
  targetId: string;
  label?: string | null;
  sourcePath: string;
  endpointId: string;
  enabled: boolean;

  state: "idle" | "running" | "failed" | "stale";

  // Realtime rate (per target; business-level bytesUploaded rate)
  up: Rate;

  // Session totals since UI start (per target; optional)
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
  "source": { "kind": "daemon", "detail": "televybackupd" },
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
