# 契约索引（#nvr79）

本目录记录本计划涉及的接口契约（按 Kind 拆分）。后续实现与测试以这些契约为准。

- [config.md](./config.md)：开发期绕过 Keychain 的配置项（env vars）
- [file-formats.md](./file-formats.md)：`vault.key` 文件格式与默认路径
- [rpc.md](./rpc.md)：daemon control IPC（CLI/macOS app 不直接访问 Keychain）
