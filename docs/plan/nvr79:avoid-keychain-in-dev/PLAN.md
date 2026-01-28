# 开发期绕过 Keychain（codesign + vault key）（#nvr79）

## 状态

- Status: 部分完成（3/4）
- Created: 2026-01-28
- Last: 2026-01-28

## 背景 / 问题陈述

- 当前仓库在两个环节与 macOS Keychain 发生交互：
  - 构建（`scripts/macos/build-app.sh`）：自动探测 codesign identity 并签名，会触发对 Keychain 中证书/私钥的访问（开发期容易反复弹窗）。
  - 运行（daemon）：Keychain 保存 vault key（`televybackup.vault_key`），用于解密本地加密 secrets store（`secrets.enc`）。开发期频繁重编译/重签名可能导致 Keychain 访问提示频繁出现。
- 期望在“开发环境”提供一套可控的绕过方案：通过一个参数**强制选择不使用 Keychain** 的保存方式，避免流程被 Keychain 权限弹窗卡住；该参数不做任何使用限制（谁都可以开，风险自担）。
- 额外约束（架构口径）：CLI 与 macOS app **不应直接访问 Keychain**。但当前代码中：
  - CLI 存在直接读写 Keychain 的 vault key 逻辑（`crates/cli/src/main.rs`）。
  - macOS app 存在直接查询 Keychain item 的逻辑（`macos/TelevyBackupApp/SettingsWindow.swift`）。
  本计划将把“Keychain + vault key + secrets store”的读写边界收敛为 daemon-only，修正上述偏离。

## 目标 / 非目标

### Goals

- 提供一个参数，可强制选择**不使用 Keychain** 的方案保存 vault key，并用于 daemon 解密/运行（开发期默认使用该参数）。
- 默认行为保持不变：未显式开启该参数时，仍使用 Keychain 保存 vault key。
- 为开发期提供可复现、低摩擦的构建方式，避免因 codesign identity 导致 Keychain 弹窗与阻塞。
- 收敛边界：Keychain / `vault.key` / `secrets.enc` 的读写由 daemon 负责；CLI 与 macOS app 不直接访问 Keychain。

### Non-goals

- 移除生产 Keychain 依赖或改变现有 secrets 安全边界（生产默认仍是“Keychain 仅 1 条 vault key + 本地加密 secrets store”）。
- 引入新的第三方 secrets 管理依赖/服务（例如 1Password/Vault 等）。
- 提供完整的 codesign 发行链路与 notarization（不属于开发期绕过的交付范围）。
- 强制在 UI 上阻止该模式（该参数无使用限制；UI 最多做风险提示）。

## 范围（Scope）

### In scope

- Dev 构建绕过（无需 Keychain 访问签名证书）：
  - 明确并文档化如何强制使用 ad-hoc 签名（例如设置 `TELEVYBACKUP_CODESIGN_IDENTITY=-`）。
- Dev 运行绕过（无需 Keychain vault key；daemon 权威）：
  - 定义 vault key 的非 Keychain 保存方式（文件），并提供“禁用 Keychain”开关，保证不会意外触发 Keychain。
  - daemon 在该模式下负责创建/读取/更新 `vault.key`，避免任何交互中断。
  - macOS app 是否增加“风险提示 icon + tooltip”由实现阶段评估并选择是否纳入（不影响核心交付）。
- 收敛边界（daemon-only）：
  - daemon 负责 Keychain 与 `vault.key` 的读写，以及 `secrets.enc` 的解密/写回。
  - CLI 与 macOS app 通过 daemon 的 IPC/RPC 获取“presence/状态/执行写入动作”的能力，不直接读写 Keychain。

### Out of scope

- 生产 secrets 方案重做（例如改为纯文件/纯硬件密钥等）。
- 旧版本 Keychain 多 items 的迁移策略变更（现有 `secrets migrate-keychain` 不在本计划内改动）。

## 需求（Requirements）

### MUST

- 开发期构建绕过：
  - 给出“无需 Keychain 的签名方式”的明确操作口径（以仓库脚本与环境变量为准）。
- 开发期运行绕过（vault key；不经 Keychain 保存）：
  - 提供一个参数 `TELEVYBACKUP_DISABLE_KEYCHAIN=1`：开启后，daemon **不允许** read/write Keychain（无使用限制）。
  - 该模式下 vault key 必须保存为文件（默认路径确定为 `TELEVYBACKUP_CONFIG_DIR/vault.key`；可通过 `TELEVYBACKUP_VAULT_KEY_FILE` 覆盖）。
  - 无中断：当 `vault.key` 不存在时，daemon 必须自动生成并写入文件后继续运行（无交互提示）。
  - 允许通过 `TELEVYBACKUP_VAULT_KEY_B64` 注入 vault key；当启用禁用 Keychain 时，如设置了该值，应写入 `vault.key` 以持久化（避免每次启动都要注入 env）。
  - 安全约束：不得把 vault key / 解密后的 secrets 明文写入日志；不得把 vault key 明文写入 `config.toml`。
- 边界（权限与调用方式）：
  - 以 daemon 为权威：只有 daemon 负责读写 vault key 保存介质（Keychain 或 `vault.key`）以及 `secrets.enc`。
  - CLI 与 macOS app 不直接访问 Keychain；需要的能力通过 daemon IPC/RPC 完成（presence 查询、写入动作、错误可操作提示）。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Daemon control IPC | RPC | internal | New | ./contracts/rpc.md | daemon | CLI/macOS app | 仅暴露“presence/状态/写入动作”，不暴露 vault key 明文 |
| `TELEVYBACKUP_DISABLE_KEYCHAIN` | Config | internal | New | ./contracts/config.md | daemon | daemon | `1` 时禁止 Keychain，并启用 `vault.key` 保存 |
| `TELEVYBACKUP_VAULT_KEY_B64` | Config | internal | New | ./contracts/config.md | daemon | daemon | Base64 32 bytes（启用禁用 Keychain 时会写入 `vault.key`） |
| `TELEVYBACKUP_VAULT_KEY_FILE` | Config | internal | New | ./contracts/config.md | daemon | daemon | vault key 文件路径（Base64 32 bytes） |
| `vault.key` | File format | internal | New | ./contracts/file-formats.md | daemon | daemon | 默认：`TELEVYBACKUP_CONFIG_DIR/vault.key` |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/config.md](./contracts/config.md)
- [contracts/file-formats.md](./contracts/file-formats.md)
- [contracts/rpc.md](./contracts/rpc.md)

## 验收标准（Acceptance Criteria）

- Given 开发者希望构建时不访问 Keychain 签名证书，
  When 使用 `TELEVYBACKUP_CODESIGN_IDENTITY=-` 执行 `./scripts/macos/build-app.sh`（或 `run-app.sh`），
  Then 构建过程不再触发 `security find-identity` 相关的 Keychain 授权弹窗（仍可生成可运行的 `.app`）。
- Given `TELEVYBACKUP_DISABLE_KEYCHAIN=1`，
  When 启动 daemon，
  Then daemon 不会尝试 read/write Keychain，并使用 `vault.key` 作为 vault key 的保存介质。
- Given `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 且 `vault.key` 不存在，
  When 启动 daemon 并需要读取 `secrets.enc`，
  Then daemon 自动生成 vault key 并写入 `vault.key` 后继续运行（无交互提示）；日志中不出现 vault key 或 secrets 明文。
- Given `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 且设置了 `TELEVYBACKUP_VAULT_KEY_B64`，
  When 启动 daemon，
  Then daemon 使用该 vault key 并将其写入 `vault.key` 以持久化（无交互提示）。
- Given 未设置任何绕过相关配置，
  When 启动 daemon，
  Then 行为与当前版本一致（继续使用 Keychain vault key）。
- Given macOS app 与 CLI 的日常路径（Settings reload / secrets set / status stream），
  When 用户在开发期使用（含 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 与默认模式），
  Then Keychain 的直接访问仅发生在 daemon；CLI 与 macOS app 不直接调用 Keychain API，仍能获得所需的“presence/状态/写入动作”能力（通过 daemon IPC/RPC）。

## 实现前置条件（Definition of Ready / Preconditions）

- 契约文档定稿（`./contracts/*.md`）后，才允许将 Status 置为 `待实现` 并进入 `/prompts:impl`。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: 覆盖 vault key source 的优先级与校验（env/file/keychain）、以及 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 的行为。
- Integration tests: 如仓库已有相关测试框架，则补一条“在禁用 Keychain 时启动并读写 secrets store”的最小集成验证（不新增框架）。

### Quality checks

- 保持仓库现有 lint/format/typecheck 约定，不引入新工具。

## 文档更新（Docs to Update）

- `README.md`: Development 增加“绕过 Keychain（codesign）”与“禁用 Keychain 时 vault key 文件保存（安全性降级）”的说明，并给出开发期默认启用的建议用法。
- `docs/architecture.md`: 补充“禁用 Keychain 时的 vault key 文件保存（安全性降级）”的边界与风险说明。
- `docs/requirements.md`: 明确“生产默认 Keychain”与“开发可绕过”的口径差异（避免误用到生产）。

## 资产晋升（Asset promotion）

None

## 实现里程碑（Milestones）

- [x] M1: 定稿 `contracts/config.md` 与 `contracts/file-formats.md`，并补充 `README.md`/`docs/*` 的开发口径说明
- [x] M2: 实现 vault key backend 切换（Keychain vs `vault.key`）与 `TELEVYBACKUP_DISABLE_KEYCHAIN` 行为（daemon）
- [x] M3: 实现 daemon control IPC，并让 CLI/macOS app 通过该 IPC 完成“presence/状态/写入动作”，移除其直接 Keychain 访问
- [ ] M4: 补齐测试与失败场景（`vault.key` 缺失自动创建、非法 Base64/长度、权限/IO 错误、`TELEVYBACKUP_VAULT_KEY_B64` 持久化写入失败、IPC 不可用/超时等）

## 方案概述（Approach, high-level）

- 以“vault backend/provider”集中处理：默认 Keychain；当 `TELEVYBACKUP_DISABLE_KEYCHAIN=1` 时，使用 `vault.key` 文件并禁止 Keychain 分支。
- 构建绕过不引入新接口：复用现有 `TELEVYBACKUP_CODESIGN_IDENTITY`，文档化推荐值 `-`（ad-hoc）。
- 文件方案以最小表面积落地：默认路径为 `TELEVYBACKUP_CONFIG_DIR/vault.key`，但不在 `config.toml` 中存储明文 key。
- 对外只暴露“控制面”（presence/写入动作），避免在 IPC 中传输 vault key 明文；daemon 负责 secrets store 的解密/写回。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：
  - vault key 落盘（`vault.key`）会降低安全性；需要明确风险提示与合理的权限/忽略策略。
  - macOS app 的签名/权限与 Keychain 交互关系较复杂；需要验证是否能做到“完全不访问 Keychain”。
- 假设（需主人确认）：
  - 主人接受：默认仍用 Keychain；但该参数不做限制，任何场景都可显式启用（风险自担）。

## 变更记录（Change log）

- 2026-01-28: 创建计划，确认范围：提供 `TELEVYBACKUP_DISABLE_KEYCHAIN` 强制使用 `vault.key`，默认路径 `TELEVYBACKUP_CONFIG_DIR/vault.key`，并要求无交互中断（缺失自动创建）。
- 2026-01-28: 冻结口径：收敛为 daemon-only（CLI/macOS app 不直接访问 Keychain），并新增 daemon control IPC（见 `contracts/rpc.md`）。
- 2026-01-28: 完成 M1（contracts + docs 口径补齐）。
- 2026-01-28: 完成 M2（daemon 支持 `vault.key` backend + `TELEVYBACKUP_DISABLE_KEYCHAIN`）。
- 2026-01-28: 完成 M3（daemon control IPC + CLI 路由 secrets 操作）。

## 参考（References）

- `scripts/macos/build-app.sh`
- `README.md`
- `docs/architecture.md`
