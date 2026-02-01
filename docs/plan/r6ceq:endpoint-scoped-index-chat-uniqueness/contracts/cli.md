# 命令行（CLI）

> Kind: CLI（internal）

本计划会影响 `backup`/`restore`/`verify`/`index sync` 等命令的 db_path 选择与 remote index 语义（按 endpoint 拆库）。

### High-level rules（frozen）

- backup：
  - 对 target，使用其 `endpoint_id` 选择 local index：`TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`
  - 上传 remote index 时仅上传该 endpoint 的 local index（不包含其它 endpoints）
- restore/verify：
  - 通过 `remote_indexes.provider` 解析 endpoint_id（`telegram.mtproto/<endpoint_id>`），选择对应 local index DB
- index sync（#0012）：
  - 必须按 endpoint 维度对齐本地 index（而不是全局单库）

（具体命令行参数/输出 schema 如需变更，在实现阶段补齐到本文件。）
