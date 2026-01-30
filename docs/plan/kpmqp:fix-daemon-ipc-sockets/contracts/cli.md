# CLI Contracts（`settings get --with-secrets`）

> Kind: CLI（internal）
>
> Binary: `televybackup`
>
> Transport: local process argv + stdout (JSON)

本计划不改变 CLI 参数与输出字段；这里只把“可用性与错误态口径”冻结为可验证契约。

## Command

- `televybackup --json settings get --with-secrets`

## Output (JSON)

### Success（IPC 可用）

- `settings`: object（v2 settings）
- `secrets`: object（presence 结构；不含明文）
- MUST: 不得出现 `secretsError`

### Degraded（IPC 不可用）

- `settings`: object（仍然可读）
- `secrets`: `null`
- `secretsError`: object
  - `code: string`（例如 `control.unavailable` / `control.timeout`）
  - `message: string`
  - `retryable: bool`

约束：

- “IPC 不可用”不得被等价为“密钥缺失”；UI 必须据此展示为 `Unavailable`（见本计划 `PLAN.md`）。
