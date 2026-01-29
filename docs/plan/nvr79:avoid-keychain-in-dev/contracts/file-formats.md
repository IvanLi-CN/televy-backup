# File format 契约：`vault.key`（#nvr79）

## 目的

在禁用 Keychain 时，用本地文件承载 vault key（用于解密 `secrets.enc`），避免 Keychain 权限弹窗导致流程卡住。

## Path / Naming

默认路径已确认：

- Default: `TELEVYBACKUP_CONFIG_DIR/vault.key`
- Override: `TELEVYBACKUP_VAULT_KEY_FILE=<path>`（见 `config.md`）

## Encoding

- Text file（UTF-8）
- Content: Base64（标准 Base64）
  - 允许首尾空白；读取时应 `trim()`
  - 解码后必须为 32 bytes

## Permissions (notes)

- 安全性降级：应尽量限制文件权限（例如仅当前用户可读写）。
- 不得提交到 Git；如需要仓库级忽略规则，由实现阶段按仓库既有约定决定是否补充（本计划不在 plan 阶段修改实现相关配置）。
- 读写边界：在本计划口径下，`vault.key` 由 daemon 创建/读写；CLI/macOS app 不直接读写（通过 daemon IPC/RPC 获取所需能力）。

## Example

```text
<BASE64_32_BYTES_HERE>
```
