# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 契约文档定稿（`./contracts/*.md`）后，才允许将 Status 置为 `待实现` 并进入 `/prompts:impl`。


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: 覆盖 vault key source 的优先级与校验（env/file/keychain）、以及 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 的行为。
- Integration tests: 如仓库已有相关测试框架，则补一条“在禁用 Keychain 时启动并读写 secrets store”的最小集成验证（不新增框架）。

### Quality checks

- 保持仓库现有 lint/format/typecheck 约定，不引入新工具。


## 实现里程碑（Milestones）

- [x] M1: 定稿 `contracts/config.md` 与 `contracts/file-formats.md`，并补充 `README.md`/`docs/*` 的开发口径说明
- [x] M2: 实现 vault key backend 切换（Keychain vs `vault.key`）与 `TELEVYBACKUP_DISABLE_KEYCHAIN` 行为（daemon）
- [x] M3: 实现 daemon control IPC，并让 CLI/macOS app 通过该 IPC 完成“presence/状态/写入动作”，移除其直接 Keychain 访问
- [x] M4: 补齐测试与失败场景（`vault.key` 缺失自动创建、非法 Base64/长度、权限/IO 错误、`TELEVYBACKUP_VAULT_KEY_B64` 持久化写入失败、IPC 不可用/超时等）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/nvr79:avoid-keychain-in-dev/PLAN.md`.
