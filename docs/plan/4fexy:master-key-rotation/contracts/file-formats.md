# 文件格式（File formats）

> Kind: File format（internal）

## Rotation state（persistent, no plaintext secrets）

- Scope: internal
- Change: New

### Location

- `TELEVYBACKUP_CONFIG_DIR/rotation/master-key.json`（示例；最终位置在实现阶段可微调，但需稳定）

### Schema (JSON)

```json
{
  "version": 1,
  "state": "idle|staged|running|paused|cancelled|completed",
  "requestedAction": "none|pause|cancel",
  "createdAt": "2026-01-31T00:00:00Z",
  "updatedAt": "2026-01-31T00:00:00Z",
  "pendingCatalogObjectIdByEndpoint": {
    "ep_a": "tgmtproto:v1:...",
    "ep_b": "tgmtproto:v1:..."
  },
  "targets": [
    {
      "targetId": "t1",
      "endpointId": "ep_a",
      "state": "pending|running|done|failed",
      "newWorld": { "snapshotId": "snp_...", "manifestObjectId": "tgmtproto:v1:..." },
      "lastError": { "code": "telegram.unavailable", "message": "..." }
    }
  ]
}
```

约束：

- rotation state 文件不得包含 master key 明文；master key（old/new）必须存放在 secrets store（加密）。
- `requestedAction` 用于跨进程控制（实现 pause/cancel）：当运行中的轮换任务检测到 `pause|cancel` 时，应在安全检查点（例如完成一个文件/pack 上传后）尽快停下，并将 `state` 置为 `paused|cancelled`。
- `pendingCatalogObjectIdByEndpoint` 用于“新世界 catalog 的 unpinned 指针”：轮换期间每个 endpoint 都会生成/更新一个 catalog 文档但不 pin；其 object_id 仅记录在本地 rotation state，commit 时才设置为 pinned。

## Index DB dual-track paths（依赖 #r6ceq）

- Current: `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`
- Next: `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite.next`
- Backup (on commit): `index.<endpoint_id>.sqlite.bak.rotated.<timestamp>`
