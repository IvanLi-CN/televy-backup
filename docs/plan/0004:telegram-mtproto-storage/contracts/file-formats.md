# File Formats Contracts（vault + secrets store）

> Kind: File format（internal）

本计划引入“Keychain 仅 1 条 vault key + 本地加密 secrets store”的存储方式，用于承载：

- Telegram bot token
- Telegram App `api_hash`
- MTProto session
- master key（`televybackup.master_key`）

## 1) Keychain：vault key（1 item）

- Service: `TelevyBackup`（与现有 Keychain service 一致；由实现决定常量）
- Account/key: `televybackup.vault_key`
- Value: Base64(32-byte random)
- 权限：最小化；默认仅本应用/CLI 可读写

约束：

- Keychain **只允许**保存这一条 vault key。
- 历史 Keychain items（如 `telegram.bot_token`、`televybackup.master_key`）应在迁移完成后删除。

## 2) 本地 secrets store：`secrets.enc`

### 位置（Path）

- 目录：`$TELEVYBACKUP_CONFIG_DIR`（未设置时为 `~/Library/Application Support/TelevyBackup/`）
- 文件：`secrets.enc`

权限约束：

- 文件必须以 owner-only 权限创建（例如 `0600`）。

### 加密封装（Encryption framing）

- `version`: 1 byte（固定 `0x01`）
- `nonce`: 24 bytes（随机）
- `ciphertext_and_tag`: 剩余全部字节（AEAD 输出）

算法：

- `enc_alg`: `xchacha20poly1305`
- key: vault key（从 Keychain 读取）
- AD（Associated Data）：固定字符串 `televybackup.secrets.v1`（UTF-8）

### 明文载荷（Plaintext schema, v1）

明文为 UTF-8 JSON：

```json
{
  "version": 1,
  "entries": {
    "telegram.bot_token": "...",
    "televybackup.master_key": "base64...",
    "telegram.mtproto.api_hash": "...",
    "telegram.mtproto.session": "base64..."
  }
}
```

说明：

- `entries` 的 value 统一为 string：
  - 二进制数据（master key、mtproto session）使用 Base64 编码保存。
- 兼容性：
  - 新增 `entries.*` key 允许向后兼容；
  - 删除/重命名 key 需要明确迁移策略（例如双写/回填/弃用周期）。

## 3) 一次性迁移（Keychain → secrets store）

当检测到历史 Keychain items 存在且 secrets store 不完整时，应执行一次性迁移：

1. 读取旧 Keychain：
   - `telegram.bot_token_key` 对应的 item（默认 `telegram.bot_token`）
   - `televybackup.master_key`
2. 写入 secrets store（同名 entry key）
3. 删除旧 Keychain items（保留 `televybackup.vault_key`）

迁移的 CLI 入口见：`./cli.md`（`televybackup secrets migrate-keychain`）。
