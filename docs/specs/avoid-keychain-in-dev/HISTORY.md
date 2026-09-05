# History

## Provenance

- Legacy source: `docs/plan/nvr79:avoid-keychain-in-dev/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `README.md`: Development 增加“绕过 Keychain（codesign）”与“禁用 Keychain 时 vault key 文件保存（安全性降级）”的说明，并给出开发期默认启用的建议用法。
- `docs/architecture.md`: 补充“禁用 Keychain 时的 vault key 文件保存（安全性降级）”的边界与风险说明。
- `docs/requirements.md`: 明确“生产默认 Keychain”与“开发可绕过”的口径差异（避免误用到生产）。


## 资产晋升（Asset promotion）

None


## 方案概述（Approach, high-level）

- 以“vault backend/provider”集中处理：默认 Keychain；当 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 时，使用 `vault.key` 文件并禁止 Keychain 分支。
- 构建绕过不引入新接口：复用现有 `TELEVYBACKUP_CODESIGN_IDENTITY`，文档化推荐值 `-`（ad-hoc）。
- 文件方案以最小表面积落地：默认路径为 `TELEVYBACKUP_CONFIG_DIR/vault.key`，但不在 `config.toml` 中存储明文 key。
- 对外只暴露“控制面”（presence/写入动作），避免在 IPC 中传输 vault key 明文；daemon 负责 secrets store 的解密/写回。


## 变更记录（Change log）

- 2026-01-28: 创建计划，确认范围：提供 `TELEVYBACKUP_DISABLE_KEYCHAIN` 强制使用 `vault.key`，默认路径 `TELEVYBACKUP_CONFIG_DIR/vault.key`，并要求无交互中断（缺失自动创建）。
- 2026-01-28: 冻结口径：收敛为 daemon-only（CLI/macOS app 不直接访问 Keychain），并新增 daemon control IPC（见 `contracts/rpc.md`）。
- 2026-01-28: 完成 M1（contracts + docs 口径补齐）。
- 2026-01-28: 完成 M2（daemon 支持 `vault.key` backend + `TELEVYBACKUP_DISABLE_KEYCHAIN`）。
- 2026-01-28: 完成 M3（daemon control IPC + CLI 路由 secrets 操作）。
- 2026-01-28: 完成 M4（补齐 `vault.key` 与 control IPC 失败场景测试）。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/nvr79:avoid-keychain-in-dev/PLAN.md`.
