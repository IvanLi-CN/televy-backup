# Settings：配置整包导出/导入（keyed config bundle）（#fn4ny）

## 状态

- Status: 已完成
- Created: 2026-01-31
- Last: 2026-02-03

## 背景 / 问题陈述

- 当前跨设备恢复与“配置丢失后重新开始”依赖多份信息：Settings（targets/endpoints/schedule 等）+ secrets（master key/TBK1、bot token、MTProto 凭据与 session 等）。
- 当 `config_dir` 丢失/重置后，用户需要手工重建上述信息；同时还需要自行判断本地 `data_dir` 与 Telegram 远端（pinned bootstrap/catalog + latest index）之间是否一致，容易误操作（覆盖/重复上传/无法继续增量）。
- 需要在 GUI 上提供“可人类搬运的整包导出/导入”，并在导入时提供预检与冲突处理，以便安全恢复或重新开始备份。
- 现状补充：本地索引库是单文件 `TELEVYBACKUP_DATA_DIR/index/index.sqlite`（全局 DB）。目前每次 backup 完成后上传的 “remote index” 实际上传的是**整份**本地 SQLite 文件（zstd 压缩 + framing 加密后分片上传），并通过所选 endpoint 的 Telegram chat 存储；这会导致“多个 endpoints/chats 上出现包含全局数据的索引”。该行为将由计划 `#r6ceq` 改为“按 endpoint 隔离索引 + 禁止 chat 复用”。

## 目标 / 非目标

### Goals

- 在 macOS Settings（Backup Config tab）提供 “Export/Import Config Bundle”：
  - 导出：保存到文件；
  - 导入：选择文件后先展示摘要，再允许用户确认应用。
- Config Bundle 应覆盖“可恢复一套可用备份配置”所需的内容：Settings v2 + 与 Settings 引用的 secrets（详见 Requirements）。
- Config Bundle 必须“自包含可导入”：在全新环境里只要导入这一个文件就能导入（无需先单独导入 TBK1）。
- 导入时必须先列出 Config Bundle 内的信息摘要：
  - 全局设置（schedule/retention/chunking/telegram.mtproto）；
  - endpoints 列表；
  - targets 列表（含 source_path/endpoint 绑定）。
- 导入时允许用户选择要恢复的 targets（默认全选），以便跳过不想恢复或存在冲突的目标。
- 导入预检（per target）至少覆盖：
  - `source_path` 是否存在；
  - 本地 `data_dir` 是否已有该 target 的索引/远端指针记录；
  - Telegram pinned bootstrap/catalog 是否存在且可解密，并能否定位到远端 latest（`snapshot_id + manifest_object_id`）。
- 当预检发现不一致/冲突时，必须让用户对每个冲突 target 选择一种动作：
  - 覆盖本地（local ← remote）：将本地索引/指针与远端 latest 对齐；
  - 覆盖远端（remote ← local）：将远端 pinned 指针更新为本地选择的 head；
  - 绑定到其他路径（rebind）：更新该 target 的 `source_path`（必要时同时更新 bootstrap/catalog 的匹配策略），并允许跳过该 target。
- 安全性：Bundle 必须加密；导入/预检过程中不得把 secrets 明文写入日志/事件输出。
- 导入 apply 后必须重建本地索引库（依赖 `#r6ceq` 的 per-endpoint 索引库方案）：
  - 对每个被导入/恢复且仍在使用的 endpoint：若存在 `index.<endpoint_id>.sqlite`，必须先原子改名为备份（临时保留），避免误覆盖且便于回滚；
  - 再按该 endpoint 的远端 latest（或空库）重建 `index.<endpoint_id>.sqlite`，用于后续 backup/restore 的增量去重判断；
  - 迁移期遗留的旧全局 `TELEVYBACKUP_DATA_DIR/index/index.sqlite`：保持静默、不读取/不写入；当 per-endpoint 索引库对“仍在使用的 endpoints”均可用后自动删除（静默；由 `#r6ceq` 定义）。

### Non-goals

- 不提供“删除 Telegram 远端历史对象/快照”的能力（最多只调整 pinned 指针或重新生成 catalog）。
- 不在本计划内解决“多设备并发备份”导致的 latest 频繁变化问题（仅在导入时提供显式决策）。
- 不更改既有 TBK1（gold key）格式与导入/导出流程。

## 范围（Scope）

### In scope

- 新增 Config Bundle key string（版本化、可保存文件）与加密封装。
- CLI 增加导出/导入（含 dry-run 预检）接口，供 GUI 调用（避免 GUI 直接拼装底层数据结构）。
- macOS SettingsWindow（`Backup Config`）页增加 UI：
  - Export bundle（save file）
  - Import bundle（sheet：inspect → select targets → resolve conflicts → apply）
- 导入预检与冲突决策模型（以 JSON 输出给 GUI；apply 时由 GUI 传回决策）。

### Out of scope

- 不新增/升级依赖（按仓库既有实现完成）。
- 不做 UI 自动化测试框架引入（若仓库已有则按现状补齐）。

## 需求（Requirements）

### MUST

#### 1) Bundle 覆盖范围（Settings + secrets）

- Bundle 必须包含 `SettingsV2` 的完整内容（schema version=2）。
- Bundle 必须包含所有被 `SettingsV2` 引用的 secrets key/value（若缺失则显式记录为缺失，并在导入摘要中展示）；其中：
  - master key 的材料通过 bundle 的 TBK1（outer `goldKey`）承载，导入时落盘为 `televybackup.master_key`（payload 内不重复携带 master key value）
  - `settings.telegram.mtproto.api_hash_key`
  - `settings.telegram_endpoints[*].bot_token_key`
- Bundle **不得**导出 MTProto session（`settings.telegram_endpoints[*].mtproto.session_key` 对应的 secrets entry value）。导入后由程序按需重新生成并落盘。

#### 2) Bundle 格式与加密

- Bundle 以单行 key string 表示（用于保存到文件；UI 不暴露 key 本体）。
- Bundle 必须版本化，并避免与 TBK1 冲突（详见 `./contracts/file-formats.md`）。
- Bundle 必须“自包含可导入”，因此该 key 内必须包含 TBK1（用于在导入时得到 master key）。
- Bundle 必须加密并要求用户提供 passphrase（PIN/password）：
  - passphrase 解锁 bundle 内的 `TBK1`（见 `./contracts/file-formats.md`）；
  - payload 明文在导出时使用 master key 做 framing 加密（`encrypt_framed`），并封装进 `TBC2` key（base64url-no-pad）。

#### 3) 导入：先 inspect，再 apply（分两步）

- 导入必须支持 dry-run/inspect：解析 bundle 并输出摘要与预检结果，不写入任何本地文件。
- 导入 apply 必须要求 GUI 显式提供：
  - 被选中的 target 列表（默认全选）；
  - 对每个冲突 target 的 resolution（overwrite local / overwrite remote / rebind / skip）。
- 导入 apply 的默认语义（frozen）：**merge**
  - bundle 中“被选择恢复”的 targets/endpoints 将 upsert 到本机 settings（同 ID 覆盖字段）。
  - 本机 settings 中“bundle 未涉及”的 targets/endpoints 不删除、不修改（保留本机额外配置）。
  - 全局 settings（例如 schedule/retention/chunking/telegram.mtproto）：以 bundle 为准覆盖本机对应字段（视为“恢复配置”的一部分）。

#### 4) 预检：目录存在性 + 远端一致性 + 本地状态

- 若某个选中的 target 的 `source_path` 不存在：该 target 必须被标记为 `missing_path`，并在 GUI 中要求用户选择 rebind 或 skip。
- 若 Telegram pinned bootstrap/catalog 缺失：对该 endpoint 下的 targets 标记为 `bootstrap_missing`（允许导入，但会影响“继续增量”的能力）。
- 若 bootstrap/catalog 存在但无法解密：标记为 `bootstrap_invalid`（需用户先导入正确 TBK1；不得自动覆盖 pinned 指针）。
- 若 bootstrap 可用且能定位远端 latest：需要给出一个“本地是否与远端 latest 一致”的判定结果与证据字段（例如 `remote_latest.snapshot_id` / `manifest_object_id` / 本地 DB 是否包含相同记录），用于驱动冲突分支（与计划 #0012 的一致性判定保持语义一致）。

#### 5) 覆盖策略语义（高风险动作必须显式确认）

- overwrite local（local ← remote）：仅调整本地索引/指针到远端 latest（必要时下载远端 latest index 并原子替换本地 DB）；不得删除远端对象。
- overwrite remote（remote ← local）：仅更新远端 pinned bootstrap/catalog 的 `latest` 指针到本地选定 head（足够）；不得删除远端对象；必须要求 GUI 二次确认（destructive 风险提示）。
- rebind：仅更新导入后 settings 中该 target 的 `source_path`；不得在导入阶段移动/复制用户的真实目录内容。

#### 6) 导入 apply：索引重建与本地备份

- apply 在写入 settings/secrets 前，必须先做本地索引预处理：
  - 对每个被恢复的 endpoint，按 `#r6ceq` 冻结的 local index layout 定位其对应的本地 index DB；
  - 若对应 DB 存在：改名为 `.bak.<timestamp>`（UTC `YYYYMMDD-HHMMSS`），并在输出中返回备份路径；
  - 若存在临时文件（例如 `.tmp`）：不得覆盖；应提示用户清理或自动使用新文件名避免冲突。
  - 迁移期兼容：若旧全局 `TELEVYBACKUP_DATA_DIR/index/index.sqlite` 存在，必须保持静默且不读取/不写入（仅保留为遗留文件）。
- apply 执行重建：
  - 默认策略：对每个 endpoint，下载其远端 latest index 并落盘为该 endpoint 的本地 index DB（与 #0012 的 remote-first index sync 语义一致，采用原子替换写入）。
  - 若某 endpoint 无法定位任何远端 latest（bootstrap missing）：为该 endpoint 生成一个空的新索引库（schema 初始化），并在输出中标注“将从首次 backup 开始逐步建立索引”。
  - 重建完成后：按 `#r6ceq` 的“自动清理”规则，旧全局 `index.sqlite` 将在条件满足时被静默删除。

#### 7) 导入 apply：风险告知与二次确认（frozen）

- apply 前必须展示“即将发生的变更摘要 + 风险提示”，并要求用户二次确认后才允许写入：
  - 写入范围：settings、secrets（包含 master key）、per-endpoint 索引库重建、以及（可选）更新远端 pinned latest 指针。
  - 风险提示至少包含：
    - 覆盖本机 master key 会改变本机对既有远端对象的解密能力（若 key 不匹配将导致 restore/verify 失败）。
    - overwrite remote 会改变跨设备 latest 指针语义（但不删除远端对象）。
    - 索引重建会改名备份旧 per-endpoint DB，并可能触发后续备份行为变化（例如重新扫描/对齐）。
    - 旧全局 `index.sqlite` 在迁移完成后会被静默删除（本地缓存清理）。
  - 二次确认形式（frozen）：typed phrase，用户必须输入 `IMPORT`。

#### 8) Master key 冲突处理（frozen）

系统只允许存在一个 master key（`televybackup.master_key`）。因此导入 bundle 时必须处理“本机已存在 master key 且与 bundle 内 TBK1 不一致”的情况：

- dry-run 必须检测并展示 master key 状态（不暴露 key 内容）：
  - `missing`：本机无 master key
  - `match`：本机 master key 与 bundle TBK1 派生 key 一致
  - `mismatch`：不一致
- 当状态为 `mismatch`：
  - 若本机尚未配置 targets：允许继续 apply（视为“首次导入/新世界”），但必须走风险告知 + 二次确认。
  - 若本机已配置 targets：不得直接覆盖 active master key；必须提示并引导进入“重设 master key（轮换）”流程（见计划 `#4fexy`）：
    - 轮换可暂停/继续/取消；
    - 未成功前旧 master key 仍可用；
    - 成功后切换为新 master key，并以新 key 做一次完整备份重建。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `televybackup settings export-bundle` | CLI | internal | New | ./contracts/cli.md | CLI | macOS GUI | 导出 bundle key（可 json） |
| `televybackup settings import-bundle` | CLI | internal | New | ./contracts/cli.md | CLI | macOS GUI | dry-run + apply（带 preflight） |
| `TBC2:<base64url_no_pad>` | File format | internal | New | ./contracts/file-formats.md | core | CLI/GUI | 加密的 config bundle key |
| `televybackup secrets rotate-master-key` | CLI | internal | New | （见 `#4fexy`） | CLI/daemon | macOS GUI | mismatch+targets 时进入轮换流程 |

### 契约文档

- [contracts/README.md](./contracts/README.md)
- [contracts/cli.md](./contracts/cli.md)
- [contracts/file-formats.md](./contracts/file-formats.md)

## 验收标准（Acceptance Criteria）

- Given 设备已存在 master key（TBK1）与完整 secrets，
  When 用户在 Settings > Backup Config 导出 Config Bundle，
  Then 得到一个 `TBC2:...` key，且可成功在另一设备上输入 passphrase 完成导入并生成等价的 Settings + secrets（以 `settings get --with-secrets` 的摘要一致为准）。

- Given 用户导入 Config Bundle，
  When 进入导入流程，
  Then 在任何写入发生前必须展示摘要（targets/endpoints/schedule + secrets 覆盖范围），并提供 target 多选（默认全选）。

- Given 用户进入导入 apply 阶段，
  When 系统展示风险提示并要求二次确认，
  Then 若用户未二次确认，系统不得写入 settings/secrets、不得重建索引、不得更新远端 pinned 指针。

- Given 本机已存在 master key 且与导入 bundle 的 TBK1 不一致，
  When 本机已配置 targets 且用户进入导入 apply，
  Then 不得直接 apply 覆盖 master key；必须提示进入 master key 轮换流程（`#4fexy`），并在轮换成功后再完成切换。

- Given 某个 target 的 `source_path` 在本机不存在，
  When 用户尝试 apply，
  Then 该 target 必须被要求选择 rebind 或 skip；不得静默落盘一个无效路径。

- Given bootstrap/catalog 存在且可解密，但本地索引与远端 latest 不一致，
  When 用户选择 overwrite local 并确认，
  Then 本地索引被对齐到远端 latest（原子替换），且后续备份按远端 latest 继续增量（与 #0012 的语义一致）。

- Given bootstrap/catalog 存在且可解密，但用户选择 overwrite remote 并二次确认，
  When apply 完成，
  Then 远端 pinned 指针更新为本地选定 head（不删除远端对象），并在输出中明确记录发生了 pinned 更新（含 old/new 指针）。

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认 Config Bundle 必须“自包含可导入”（只导入一个文件即可导入）。
- 已确认 overwrite remote 的语义：仅更新 pinned 指针（不删除远端对象）。
- 已确认 MTProto session 不导出：导入后按需重新生成并落盘。
- 已确认 “local-vs-remote mismatch” 的判定口径（以每个 target 对应 endpoint 的 `index.<endpoint_id>.sqlite` 记录为依据是否足够）。
- 已确认导入 apply 的“索引重建”策略：备份旧 db → 拉取远端最新可用索引落盘为新 db（或 bootstrap missing 时建空库）。
- 已确认 index 按 endpoint 隔离 + 禁止 chat 复用（见计划 `#r6ceq`），从而避免“multi-endpoint 场景下索引来源选择”的歧义。
- 已确认迁移期兼容策略：旧全局 `index.sqlite` 存在时静默忽略；导入 apply 仅处理 per-endpoint `index.<endpoint_id>.sqlite`（见 `#r6ceq`）。
- 已确认二次确认的具体交互形态：typed phrase（输入 `IMPORT`），并明确“哪些动作”需要额外的二次确认（例如 overwrite remote / overwrite master key）。
- 已确认 master key mismatch 的策略：
  - 无 targets：允许 apply（但需二次确认）
  - 有 targets：进入 `#4fexy` 的轮换流程（可暂停/继续/取消，成功后切换）

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit（Rust / core）：
  - bundle encode/decode round-trip
  - schema/version mismatch 行为
  - secrets 覆盖范围与缺失标注
- Integration（CLI）：
  - export → import(dry-run) → import(apply) 的 happy path
  - 冲突场景：missing_path / bootstrap_missing / bootstrap_invalid / local-vs-remote mismatch
- Manual（macOS GUI）：
  - 导入 UI：摘要展示、targets 默认全选、多选与冲突决策交互

### Quality checks

- `cargo fmt` / `cargo clippy` / `cargo test`（按仓库既有 CI 约束）

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

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：
  - overwrite remote 属高风险动作（可能改变跨设备“latest”语义），需要强交互确认与明确的“只更新 pinned 指针，不删除对象”的限制。
  - bundle 可能包含敏感信息（bot token 等）；必须保证加密与日志脱敏。
  - 若导入时触发 master key 轮换：需要额外的时间、带宽与远端存储（相当于“重备份一个新世界”）；commit 后若未妥善保存旧 TBK1，将无法再解密旧世界数据（见 `#4fexy`）。
- 开放问题：
  - None
- 假设（需主人确认）：
  - None

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
