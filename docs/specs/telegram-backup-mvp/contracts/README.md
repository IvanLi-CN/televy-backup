# Contracts（#0001）

本目录记录本计划涉及的所有“跨边界接口契约”。实现必须以这里为准做增量变更。

- `rpc.md`: CLI/IPC contracts（native macOS app ↔ `televybackup` CLI）
- `events.md`: progress events（`televybackup` stdout NDJSON）
- `db.md`: SQLite schema（索引与任务）
- `file-formats.md`: 本地配置/缓存/索引打包的文件格式约定

补充约定：

- 所有 secret（Bot token、主密钥）不得落盘于 `config.toml` 或日志；只允许存于 macOS Keychain，并通过 RPC 以“presence/写入”方式交互。
