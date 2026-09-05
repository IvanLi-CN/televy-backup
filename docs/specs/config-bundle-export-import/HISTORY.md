# History

## Provenance

- Legacy source: `docs/plan/fn4ny:config-bundle-export-import/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `README.md`：补充“推荐的恢复流程”：TBK1 + Config Bundle（以及导入时的冲突处理入口）。
- `docs/architecture.md`：补充 Config Bundle 的数据流、加密与导入预检语义（以及与 bootstrap/catalog 的关系）。
- `docs/design/ui/settings-window/settings-window-security.svg`：补充 Backup Config 页新增的 bundle 区块与交互（若实现阶段需要更新视觉基准）。


## 计划资产（Plan assets）

- None


## 资产晋升（Asset promotion）

None


## 方案概述（Approach, high-level）

- 复用现有基础设施：
  - framing 加密：`crates/core/src/crypto.rs`
  - TBK1/gold key：`crates/core/src/gold_key.rs` + CLI `secrets export/import-master-key`
  - pinned bootstrap/catalog：`crates/core/src/bootstrap.rs`（并与计划 #0012 的“远端 latest 判定”语义一致）
- 预期实现触点（供 impl 阶段定位，不在 plan 阶段改动）：
  - GUI：`macos/TelevyBackupApp/SettingsWindow.swift`
  - CLI：`crates/cli/src/main.rs`
  - core：新增 `config_bundle` 模块（或等价位置），承载 schema + encode/decode + preflight 模型


## 变更记录（Change log）

- 2026-01-31: 冻结决策：bundle 自包含可导入；overwrite remote 仅更新 pinned 指针（不删除远端对象）；MTProto session 不导出
- 2026-01-31: 新增导入后索引重建要求：备份旧索引库，重建新索引库并作为后续增量去重依据
- 2026-01-31: 对齐新约束：索引按 endpoint 隔离 + 禁止 chat 复用（依赖计划 `#r6ceq`）
- 2026-01-31: 冻结迁移期兼容：旧全局 `index.sqlite` 静默忽略；导入 apply 仅处理 per-endpoint 索引库
- 2026-01-31: 冻结 master key 冲突策略：mismatch 默认阻断，需显式 Rotate + 二次确认
- 2026-01-31: 调整：本计划不提供 Rotate；master key mismatch 一律阻断，建议用独立 profile
- 2026-01-31: 需求变更：master key mismatch + 已有 targets 时必须进入 `#4fexy` 轮换流程（可暂停/继续/取消）；无 targets 时允许 apply（需二次确认）
- 2026-01-31: 更新：导入 apply 的索引重建按 per-endpoint `index.<endpoint_id>.sqlite` 执行（旧全局 `index.sqlite` 静默忽略并按 `#r6ceq` 自动清理）
- 2026-01-31: 冻结导入默认语义：merge（保留本机额外 targets/endpoints；bundle 覆盖同 ID 与全局 settings）
- 2026-01-31: 冻结二次确认交互：typed phrase（输入 `IMPORT`）
- 2026-02-01: 已实现：`settings export-bundle` / `settings import-bundle`（dry-run/apply）+ macOS Settings Backup Config UI 入口 + docs 同步
- 2026-02-02: 更新：Config bundle 改为 passphrase 保护（`TBC2`；避免与 `TBK1` 同存导致单点泄露）
- 2026-02-02: 更新：Settings 的 Config 页仅保留“导出配置 / 导入配置”入口；导入时展示明文 hint
- 2026-02-03: 更新：导出改为系统保存对话框（Save Panel）选择保存位置；passphrase + 可选附言在同一对话框内填写；附言支持多行输入
- 2026-02-03: 更新：导入（预检前）界面改为紧凑空状态 + 选择文件后再输入 passphrase/查看附言；并按阶段动态调整 sheet 尺寸
- 2026-02-04: 更新：导入改为先用系统文件选择器选文件，再进入导入 sheet（减少空状态停留）；并将导入 sheet 预检/结果阶段尺寸固定为可用高度，避免结果页被裁剪
- 2026-02-04: 修复：导入后备份卡在 Running 时，MTProto upload 侧增加超时保护（helper 读响应超时/分片上传超时）；verify/restore 将 Telegram download 超时按 `telegram.unavailable` 上报（可重试），避免误报为 chunk 丢失
- 2026-02-05: 优化：导入预检/应用阶段错误提示（解析 CLI JSON 错误并将“密码错误”等常见场景映射为用户可读文案）

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/fn4ny:config-bundle-export-import/PLAN.md`.
