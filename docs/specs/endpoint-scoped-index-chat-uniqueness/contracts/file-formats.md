# 文件格式（File formats）

> Kind: File format（internal）

## Local index DB（endpoint-scoped）

- 范围（Scope）: internal
- 变更（Change）: Modify

### Goal

- 本地索引与 remote index 都必须按 endpoint/provider 隔离，避免跨 endpoint 污染与重复上传。

### Options（需要冻结其一）

#### Frozen: Option B（本地按 endpoint 拆分）

- Local DB path pattern（frozen）: `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`
  - `<endpoint_id>`：`SettingsV2.telegram_endpoints[*].id`
- Remote index upload（frozen）:
  - 对 endpoint `<endpoint_id>`，只上传其对应的 local DB（因此 remote index 自然不包含其它 endpoints）

### Temporary / backup naming（frozen）

- Import/apply 或迁移前的备份改名：
  - `index.<endpoint_id>.sqlite.bak.<timestamp>`
  - `timestamp`：UTC `YYYYMMDD-HHMMSS`
- 临时写入（原子替换）：
  - `index.<endpoint_id>.sqlite.tmp`（写入后 rename 覆盖）

### Compatibility / migration

- 从现有“全局 DB”（`index.sqlite`）迁移到 per-endpoint 多库（frozen）：
  - 不做自动拆分迁移；
  - 从“下一次备份开始”按 endpoint 新建 `index.<endpoint_id>.sqlite`；
  - 旧 `index.sqlite` 迁移期保持静默（不提示用户，不读写）；当 per-endpoint DB 对“仍在使用的 endpoints”均可用后自动删除（保持静默；定义见 `../PLAN.md`）。
