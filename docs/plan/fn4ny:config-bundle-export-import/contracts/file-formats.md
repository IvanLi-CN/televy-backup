# 文件格式（File formats）

> Kind: File format（internal）

## Config Bundle key（`TBC2:<base64url_no_pad>`）

- 范围（Scope）: internal
- 变更（Change）: New
- 编码（Encoding）: ASCII（prefix + base64url-no-pad）

### Overview

Config Bundle 用于“人类可搬运”的整包配置导出/导入：

- 目标：在配置丢失/迁移时，快速恢复 Settings + secrets，并具备导入预检与冲突处理能力。
- 安全：
  - bundle 内含敏感 secrets（bot token / api_hash 等），因此必须加密并避免明文落盘（除非用户明确保存 key 到文本文件）。
  - `TBC2` 额外要求用户提供 passphrase（PIN/password）才能解密导入，避免与 `TBK1` 同存导致“单点泄露即全盘泄露”。

### Wire format（outer）

- Prefix: `TBC2:`
- Body: base64url-no-pad（URL_SAFE_NO_PAD）编码后的 JSON bytes（UTF-8）

说明：Config Bundle 要求“自包含可导入”，但 `TBC2` 将 `TBK1` 以 passphrase 派生密钥加密存放（`goldKeyEnc`），随后使用解出的 master key 解密 payload。

### Outer JSON schema（JSON, UTF-8）

```json
{
  "version": 2,
  "format": "tbc2",
  "hint": "<string>",
  "kdf": {
    "name": "pbkdf2_hmac_sha256",
    "iterations": 200000,
    "salt": "<base64url_no_pad(random_bytes)>"
  },
  "goldKeyEnc": "<base64url_no_pad(framed_bytes)>",
  "payloadEnc": "<base64url_no_pad(framed_bytes)>"
}
```

说明：

- `hint` 为明文提示短语：导入时可在不输入 passphrase 的情况下展示，用于帮助用户确认正在使用正确的 bundle。

### Crypto framing（goldKeyEnc）

- `goldKeyEnc` 解码后为 framed bytes（framing v1）。
- 加密：`encrypt_framed(passphrase_key, AAD, plaintext_tbk1)`
- 解密：`decrypt_framed(passphrase_key, AAD, framed_bytes)`
- `passphrase_key`：PBKDF2-HMAC-SHA256(passphrase, salt, iterations) 输出 32 bytes
- `AAD`: 固定为 `televy.config.bundle.v2.gold_key`（UTF-8 bytes）

### Crypto framing（payloadEnc）

- `payloadEnc` 解码后为 framed bytes。
- 加密：`encrypt_framed(master_key, AAD, plaintext_json)`
- 解密：`decrypt_framed(master_key, AAD, framed_bytes)`
- `AAD`: 固定为 `televy.config.bundle.v2.payload`（UTF-8 bytes）

### Payload plaintext schema（JSON, UTF-8）

```json
{
  "version": 2,
  "exported_at": "2026-01-31T00:00:00Z",
  "settings": { "... SettingsV2 ..." },
  "secrets": {
    "entries": {
      "telegram.mtproto.api_hash": "<string>",
      "telegram.bot_token.ep_x": "<string>"
    },
    "excluded": [
      "telegram.mtproto.session.ep_x"
    ],
    "missing": [
      "telegram.bot_token.ep_y"
    ]
  }
}
```

约束：

- `settings` 必须为 Settings schema version=2（`SettingsV2`）。
- `secrets.entries` 仅允许包含 “被 settings 引用的 key”（不包含 master key；master key 通过 outer `goldKeyEnc`（需 passphrase）承载）。其余 key 不允许混入（避免把无关 secrets 一并导出）。
- `secrets.excluded` 用于表示“settings 引用但本计划明确不导出”的 key（当前固定为 MTProto session keys）；导入端应提示“会在首次连接时重新生成并落盘”。
- `secrets.missing` 用于表示“settings 引用且应导出，但本地未持有”的 key（导入可继续，但应提示用户后续补齐；例如某 endpoint 尚未设置 bot token）。

### Compatibility / migration

- `TBC2` 与 `version=2` 固定对应：
  - `AAD(goldKeyEnc)=televy.config.bundle.v2.gold_key`
  - `AAD(payloadEnc)=televy.config.bundle.v2.payload`
- 若未来需要变更 schema / KDF：
  - bump `TBC<next>:` 前缀与 `version`；
  - 新旧版本并存：导入端至少支持当前版本与上一个版本（具体窗口在未来计划中定义）。
