# 命令行（CLI）

> Kind: CLI（internal）

## `televybackup settings export-bundle`

- 范围（Scope）: internal
- 变更（Change）: New

### 用法（Usage）

```text
televybackup [--config-dir <path>] --json settings export-bundle [--hint "<string>"]
televybackup [--config-dir <path>] settings export-bundle [--hint "<string>"]
```

### 输入（Inputs）

- 读取本地 `config.toml`（Settings v2）
- 读取本地 secrets store（`secrets.enc`，经 vault key 解密）
- 必须存在 master key（`televybackup.master_key`），否则返回错误（见 Errors）

### 输出（Output）

- `--json`：
  - `bundleKey`: string（`TBC2:...`）
  - `format`: string（固定 `"tbc2"`）
- 非 `--json`：stdout 输出单行 `TBC2:...`

额外约束：

- `--hint` 可选（明文提示短语，导入时可见；可为空）。
- 必须提供 passphrase（PIN/password）：通过环境变量 `TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE` 传入（不落盘）。

### Errors

- `config.invalid`: master key missing / settings invalid
- `secrets.store_failed`: secrets store 读取失败（vault/keychain 不可用、格式错误等）

## `televybackup settings import-bundle --dry-run`

- 范围（Scope）: internal
- 变更（Change）: New

### 用法（Usage）

```text
televybackup [--config-dir <path>] [--data-dir <path>] --json settings import-bundle --dry-run
```

### 输入（Inputs）

- stdin：单行 `TBC2:...`
- 必须提供 passphrase：环境变量 `TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE`
- 运行时需要访问 Telegram 远端进行预检时，使用 bundle 内的 endpoint 信息与 secrets（bot token / mtproto api hash）进行访问。
  - MTProto session **不在 bundle 中导出/导入**；预检过程中如需要建立 MTProto 连接，允许生成临时 session（仅内存使用，不落盘）。

### 输出（Output, JSON）

```json
{
  "format": "tbc2",
  "localMasterKey": { "state": "missing|match|mismatch" },
  "localHasTargets": true,
  "nextAction": "apply|start_key_rotation",
  "bundle": {
    "settingsVersion": 2,
    "targets": [
      { "id": "t1", "sourcePath": "/path", "endpointId": "ep_x", "label": "..." }
    ],
    "endpoints": [
      { "id": "ep_x", "chatId": "-100...", "mode": "mtproto" }
    ],
    "secretsCoverage": {
      "presentKeys": ["..."],
      "excludedKeys": ["..."],
      "missingKeys": ["..."]
    }
  },
  "preflight": {
    "targets": [
      {
        "targetId": "t1",
        "sourcePathExists": true,
        "bootstrap": { "state": "ok|missing|invalid", "details": {} },
        "remoteLatest": { "state": "ok|missing", "snapshotId": "...", "manifestObjectId": "..." },
        "localIndex": { "state": "match|stale|missing", "details": {} },
        "conflict": { "state": "none|needs_resolution", "reasons": ["..."] }
      }
    ]
  }
}
```

### Errors

- `config.invalid`: bundle 无效 / 版本不支持
- `crypto`: 解密失败（master key 不匹配）
- `telegram.unavailable`: 预检时 Telegram 访问失败（retryable）

## `televybackup settings import-bundle --apply`

- 范围（Scope）: internal
- 变更（Change）: New

### 用法（Usage）

```text
televybackup [--config-dir <path>] [--data-dir <path>] --json settings import-bundle --apply
```

### 输入（Inputs, JSON on stdin）

```json
{
  "bundleKey": "TBC2:...",
  "selectedTargetIds": ["t1", "t2"],
  "confirm": {
    "ackRisks": true,
    "phrase": "ROTATE"
  },
  "resolutions": {
    "t1": { "mode": "overwrite_local" },
    "t2": { "mode": "rebind", "newSourcePath": "/new/path" }
  }
}
```

约束：

- `selectedTargetIds` 为空时视为错误（避免误清空 targets）。
- apply 默认语义（frozen）：merge
  - 仅对 bundle 中被选择恢复的 targets/endpoints 做 upsert（同 ID 覆盖字段）。
  - 不删除、不修改本机 settings 中“bundle 未涉及”的 targets/endpoints（保留本机额外配置）。
- 若 dry-run 发现 `conflict.state=needs_resolution` 且 apply 未提供对应 `resolutions[targetId]`：返回 `config_bundle.conflict`。
- 迁移期兼容：apply 不读取/不写入旧全局 `TELEVYBACKUP_DATA_DIR/index/index.sqlite`；仅操作 per-endpoint `index.<endpoint_id>.sqlite`（按 `#r6ceq`）。
- 必须显式确认风险（frozen）：若 `confirm.ackRisks != true` 或 `confirm.phrase != "ROTATE"`：返回 `config_bundle.confirm_required`。
- 若 dry-run 检测到 `localMasterKey.state=mismatch` 且 `localHasTargets=true`：返回 `config_bundle.rotation_required`（应走 `#4fexy` 的轮换流程）。
- 若 dry-run 检测到 `localMasterKey.state=mismatch` 且 `localHasTargets=false`：允许 apply，但必须二次确认（仍走 `confirm.*`）。

### 输出（Output, JSON）

```json
{
  "ok": true,
  "localIndex": {
    "previousDbBackupPath": "/.../index/index.<endpoint_id>.sqlite.bak.20260131-123456",
    "rebuiltDbPath": "/.../index/index.<endpoint_id>.sqlite",
    "rebuiltFrom": { "mode": "remote_latest|empty", "snapshotId": "snp_...", "manifestObjectId": "tgmtproto:v1:..." }
  },
  "applied": {
    "targets": ["t1", "t2"],
    "endpoints": ["ep_x"],
    "secretsWritten": ["..."]
  },
  "actions": {
    "updatedPinnedCatalog": [
      { "endpointId": "ep_x", "old": "...", "new": "..." }
    ],
    "localIndexSynced": [
      { "targetId": "t1", "from": "remoteLatest", "to": "local" }
    ]
  }
}
```

说明：

- `localIndex.rebuiltFrom.mode=remote_latest`：导入时从远端 latest 下载对应索引库作为新本地索引的初始内容。
- `localIndex.rebuiltFrom.mode=empty`：导入时无法定位远端 latest（或 bootstrap/catalog 缺失），因此以空库初始化，后续由首次 backup 逐步建立。

### Errors

- `config.invalid`: bundle 无效 / schema 不支持
- `config_bundle.passphrase_required`: 未提供 `TELEVYBACKUP_CONFIG_BUNDLE_PASSPHRASE`
- `config_bundle.conflict`: 缺少冲突决策
- `secrets.store_failed`: secrets store 写入失败
- `telegram.unavailable`: overwrite remote / sync local 时 Telegram 访问失败（retryable）
