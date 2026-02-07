# Import bundle: Rebind compare local vs remote latest（#w2k9p）

## 状态

- Status: 已完成
- Created: 2026-02-06
- Last: 2026-02-07

## 背景 / 问题

导入 `TBC2` 配置后，用户可能会为某个 target 重新绑定（rebind）到本机的另一个目录：

- 目录可能是空目录 / 与原目录相同 / 与原目录不同且非空。
- 如果该 target 在 Telegram 上存在 remote latest snapshot，那么 rebind 会变成一个**数据平面**问题：
  - 本地目录内容是否与 remote latest 完全一致？
  - 若不一致，应该以哪一边为准（本地 / 远端 / 合并）？

关键约束：

- **不得使用本地 index DB（SQLite）作为“目录与 remote latest 一致”的依据。**
  - 本地 index 仅是缓存/加速结构；导入/恢复时不具备权威性。

## 目标

- 当 Import bundle inspect 结果页加载完成后：
  - 对所有“被选择且存在 remote latest”的 targets，自动做一次**内容级验证**（remote index DB vs 本地文件 bytes）。
  - 若用户为某个 target 选择/更换目录（Change…），则 compare 使用新目录路径作为验证对象。
  - 只有当发现差异时，才要求用户选择冲突处理策略（本地 / 远端 / 合并）。

## 范围（In/Out）

### In scope

- CLI 新增一个 compare 命令用于 UI 调用（JSON 输入/输出）。
- compare 的“权威来源”：
  - remote index DB（从 Telegram 下载 manifest + index parts 并解密）
  - 本地目录真实文件 bytes（按 index DB 的 `file_chunks(offset,len)` 做 BLAKE3 校验）
- UI 集成：
  - 显示 compare 状态（checking / match / mismatch / error）
  - Apply 在 compare 未完成时不可用（阻塞式校验）
  - mismatch 时显示冲突处理选项：
    - **远端**：恢复 remote latest（要求目标目录为空）
    - **本地**：保留本地目录（允许未来 backup 覆盖 remote latest）
    - **合并（选项 B）**：导入后立刻跑一次 backup（local -> remote 生成新 snapshot，不依赖本地 index）
  - mismatch 且选择“远端/合并”时，Apply 成功后自动执行对应 restore/backup 动作。

### Out of scope

- 不修改主界面上的 Verify 功能（该 Verify 与本方案无关）。
- 不做“文件级交互式合并”（逐文件选择）；合并语义按选项 B 固定为 `local -> remote` 生成新快照。

## 验收标准（Acceptance Criteria）

- Given target 存在 remote latest，When Import bundle inspect 结果页展示，Then UI 会自动 compare：
  - compare == checking/unknown：Apply 不可用。
  - compare == match：不提示冲突选项，Apply 可用。
  - compare == mismatch：必须选择（本地/远端/合并）后 Apply 才可用。
  - compare == error：Apply 不可用，并提供 Retry/Check 重试。
- Given 用户为某个 target 选择/更换目录（rebind），Then compare 使用新目录路径，并忽略旧路径的 compare 结果（防止 stale）。
- Given mismatch + 选择 “Use remote latest”，Then Apply 后自动执行 `restore latest` 到该目录（且 UI 要求目录为空）。
- Given mismatch + 选择 “Merge (backup local to remote)”（选项 B），Then Apply 后自动执行一次 `backup run` 生成新 snapshot。
- compare 过程不读取/依赖本地 index DB 来判断一致性（必须以 remote index + 本地 bytes 为准）。

## 测试 / 验证

- `cargo test`（覆盖 compare 的 match/mismatch/extra/missing/hash mismatch 等单元测试）。
- `./scripts/macos/build-app.sh`（确保 Swift 编译通过）。
- UI snapshot（可选）：展示 compare 行为与冲突选项（无需依赖真实网络即可截图）。

## 里程碑（Milestones）

- [x] 核心：实现 local-vs-remote snapshot content compare（remote index DB + 本地 bytes）
- [x] CLI：暴露 `settings import-bundle --compare-folder` JSON 接口供 UI 调用
- [x] UI：inspect 后自动 compare；mismatch 才要求冲突选择；按选项执行 restore/backup
- [x] 文档：澄清 compare/冲突处理语义，强调不依赖本地 index DB
