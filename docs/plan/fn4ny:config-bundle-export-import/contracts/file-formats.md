# 文件格式（File formats）

> Kind: File format（internal）

## Config Bundle key（`TBC1:<base64url_no_pad>`）

- 范围（Scope）: internal
- 变更（Change）: New
- 编码（Encoding）: ASCII（prefix + base64url-no-pad）

### Overview

Config Bundle 用于“人类可搬运”的整包配置导出/导入：

- 目标：在配置丢失/迁移时，快速恢复 Settings + secrets，并具备导入预检与冲突处理能力。
- 安全：bundle 明文必须以 master key 做 framing 加密（`encrypt_framed`），并且不允许明文落盘（除非用户明确保存 key 到文本文件）。

### Wire format（outer）

- Prefix: `TBC1:`
- Body: base64url-no-pad（URL_SAFE_NO_PAD）编码后的 JSON bytes（UTF-8）

说明：Config Bundle 要求“自包含可导入”，因此 outer JSON 中必须包含 TBK1（用于得到 master key），以及一个经 master key 加密的 payload。

### Outer JSON schema（JSON, UTF-8）

```json
{
  "version": 1,
  "format": "tbc1",
  "goldKey": "TBK1:...",
  "payloadEnc": "<base64url_no_pad(framed_bytes)>"
}
```

### Crypto framing（payloadEnc）

- `payloadEnc` 解码后为 framed bytes。
- 加密：`encrypt_framed(master_key, AAD, plaintext_json)`
- 解密：`decrypt_framed(master_key, AAD, framed_bytes)`
- `AAD`: 固定为 `televy.config.bundle.v1`（UTF-8 bytes）

### Payload plaintext schema（JSON, UTF-8）

```json
{
  "version": 1,
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
- `secrets.entries` 仅允许包含 “被 settings 引用的 key”（不包含 master key；master key 通过 outer `goldKey` 承载）。其余 key 不允许混入（避免把无关 secrets 一并导出）。
- `secrets.excluded` 用于表示“settings 引用但本计划明确不导出”的 key（当前固定为 MTProto session keys）；导入端应提示“会在首次连接时重新生成并落盘”。
- `secrets.missing` 用于表示“settings 引用且应导出，但本地未持有”的 key（导入可继续，但应提示用户后续补齐；例如某 endpoint 尚未设置 bot token）。

### Compatibility / migration

- `TBC1` 与 `version=1` 固定对应 `AAD=televy.config.bundle.v1`。
- 若未来需要变更明文 schema：
  - bump `TBC<next>:` 前缀与 `version`；
  - 新旧版本并存：导入端至少支持当前版本与上一个版本（具体窗口在未来计划中定义）。
