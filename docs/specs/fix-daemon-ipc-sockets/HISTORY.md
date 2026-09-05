# History

## Provenance

- Legacy source: `docs/plan/kpmqp:fix-daemon-ipc-sockets/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `README.md`: 增加一段 Troubleshooting：当 Recovery Key 显示不可用/verify 报 `daemon.unavailable` 时，优先检查 daemon 是否运行以及 `TELEVYBACKUP_*_DIR` 是否一致。
- `docs/architecture.md`（如已有相关小节）：补充“IPC endpoints（control/vault/status）”的职责边界与定位入口（仅文档层面说明）。


## 资产晋升（Asset promotion）

None


## 方案概述（Approach, high-level）

- 优先修复“daemon 启动后未监听 control/vault IPC”的根因（绑定失败、残留 socket、权限/路径不一致、生命周期被提前 drop 等）。
- GUI 侧不再把“secrets 不可用（control IPC 不可用）”等价成“密钥缺失”；以 `secretsError` 作为第一手信号。
- 对 verify：在执行前做一次 daemon 可用性 preflight（或让 UI 触发 daemon 启动/重启），失败时给出可执行的下一步动作。


## 变更记录（Change log）

- 2026-01-30: 冻结关键决策（UI 文案口径 + daemon 自愈 + GUI preflight + 测试禁用 Keychain），状态切换为 `待实现`
- 2026-01-30: 完成实现（daemon IPC 启动鲁棒性 + GUI preflight/Unavailable + Troubleshooting 文档）；等待主人验收后推进后续合并/PR
- 2026-01-31: dev/automation 启动改为 `open --args` 传参并默认使用 workspace `.dev/` 目录；修复 IPC 就绪判定（避免残留 socket 假阳性）与手动备份触发路径。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/kpmqp:fix-daemon-ipc-sockets/PLAN.md`.
