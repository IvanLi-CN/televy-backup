# Config 契约：开发期绕过 Keychain（#nvr79）

## 范围

本契约定义“绕过 Keychain”的配置项（环境变量）。

- 这些配置项由 daemon 读取并生效。
- CLI 与 macOS app 不应直接访问 Keychain；需要的能力通过 daemon control IPC（见 `rpc.md`）完成。
- 默认行为不变：未显式开启时，daemon 仍使用 Keychain 保存/读取 vault key。

## Variables

### 1) `TELEVYBACKUP_DISABLE_KEYCHAIN`

- Type: string
- Values:
  - `1`：禁止任何 Keychain read/write
  - 其他/未设置：允许（默认）
- Behavior:
  - 当为 `1` 时，必须禁止任何 Keychain read/write，并强制使用文件 `vault.key` 作为 vault key 的保存介质（路径见下文）。
  - 无交互中断：
    - 若 `vault.key` 不存在且未提供 `TELEVYBACKUP_VAULT_KEY_B64`：必须自动生成 32-byte vault key，并写入 `vault.key` 后继续。
    - 若提供了 `TELEVYBACKUP_VAULT_KEY_B64`：必须使用该值，并写入 `vault.key` 以持久化后继续。
  - 本开关不做任何使用限制：谁都可以开启（风险自担）。

### 2) `TELEVYBACKUP_VAULT_KEY_B64`

- Type: string
- Format: Base64（标准 Base64，解码后必须为 32 bytes）
- Behavior:
  - 优先级最高：一旦设置且合法，则使用其作为 vault key。
  - 不得把该值写入日志或序列化到 `--json` 输出中（允许输出“已设置/未设置”的 presence）。
  - 当 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 时：必须将该值写入 `vault.key`（避免每次启动都要注入 env）。

### 3) `TELEVYBACKUP_VAULT_KEY_FILE`

- Type: string
- Meaning: vault key 文件路径（读取文件内容为 Base64；解码后必须为 32 bytes）
- Behavior:
  - 当 `TELEVYBACKUP_VAULT_KEY_B64` 未设置时，使用该文件作为来源。
  - 文件内容允许包含首尾空白（trim 后再解码）。
  - 当 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 且该文件不存在时：必须创建并写入（自动生成或写入 `TELEVYBACKUP_VAULT_KEY_B64`）。

## Precedence

1. `TELEVYBACKUP_VAULT_KEY_B64`
2. `TELEVYBACKUP_VAULT_KEY_FILE`
3. 默认文件 `vault.key`（见 `file-formats.md`；当 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 时必须启用）
4. Keychain（仅当 `TELEVYBACKUP_DISABLE_KEYCHAIN != 1` 时允许）

## Errors

（错误码命名以仓库现有约定为准；这里先给出建议形状，待实现时对齐现有错误体系）

- `secrets.vault_unavailable`: vault key 不可用（例如禁用 Keychain 且 `vault.key` 读写失败、且未提供 `TELEVYBACKUP_VAULT_KEY_B64`）
- `secrets.vault_key_invalid`: vault key Base64 无效或长度不为 32 bytes
- `keychain.disabled`: 禁用 Keychain 时仍触发了 Keychain 分支（应视为 bug）
- `secrets.vault_key_file_io_failed`: `vault.key` 读取/写入失败（权限、路径不可用等）

## Examples

```bash
# dev: 强制禁用 Keychain，并从 env 提供 vault key
export TELEVYBACKUP_DISABLE_KEYCHAIN=1
export TELEVYBACKUP_VAULT_KEY_B64='BASE64_32_BYTES_HERE'
```

```bash
# dev: 强制禁用 Keychain，并从文件提供 vault key
export TELEVYBACKUP_DISABLE_KEYCHAIN=1
export TELEVYBACKUP_VAULT_KEY_FILE="$TELEVYBACKUP_CONFIG_DIR/vault.key"
```
