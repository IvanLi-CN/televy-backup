# 索引按 Endpoint 隔离 + 禁止 chat 复用

> Canonical topic spec migrated from `docs/plan/r6ceq:endpoint-scoped-index-chat-uniqueness/PLAN.md`.
> Legacy source retained pending delete approval.

## 背景 / 问题陈述

- 当前本地索引库为单文件：`TELEVYBACKUP_DATA_DIR/index/index.sqlite`，并且每次 backup 会上传“整份 DB”作为 remote index（压缩+加密后分片上传）到当前 endpoint 的 Telegram chat。
- 当 endpoints 增多时，这会带来：
  - 重复上传：每个 endpoint/chat 都会收到一份包含“其它 endpoints 数据”的索引（虽然加密不可读，但造成带宽与存储浪费）。
  - 心智负担：用户难以理解“为什么 A chat 里包含 B endpoint 的索引内容”。
- 需要将“索引”按 endpoint 隔离：每个 endpoint 只维护属于自己的索引文档；同时禁止一个 chat 被多个 endpoints 复用。


## 目标 / 非目标

### Goals

- 每个 endpoint/chat 只上传与自身相关的索引（index manifest + parts），不包含其它 endpoints/provider 的记录。
- 配置与导入流程必须强约束：一个 chat（`chat_id`）不得被多个 endpoints 使用。
- 导入后重建索引时，按 endpoint 分别恢复对应索引（不再依赖“全局单库作为索引来源”的选择）。

### Non-goals

- 不做 Telegram 远端对象 GC。
- 不在本计划内改变 TBK1 与 secrets store 的基本模型。


## 范围（Scope）

### In scope

- Index 的“按 endpoint 隔离”策略（local layout + remote upload shape + restore/sync path）。
- Settings 校验：`telegram_endpoints[*].chat_id` 全局唯一（禁止复用）。
- 与既有 remote bootstrap/catalog（pinned）协作的行为调整（如需）。

### Out of scope

- 大规模 UI 改版（仅做必要的错误提示/导入预检展示）。


## 需求（Requirements）

### MUST

#### 1) 禁止 chat 复用

- `SettingsV2.telegram_endpoints[*].chat_id` 必须全局唯一：
  - 若发现重复：`config.invalid`（GUI 应给出明确提示，导入流程不得继续 apply）。

#### 2) Remote index 必须按 endpoint 隔离

- 对于某次 backup 使用的 endpoint `ep.id`：
  - 上传到该 endpoint/chat 的 remote index **不得包含** 其它 endpoint 的数据。
- 远端“latest index”定位仍以 pinned bootstrap/catalog 为权威（per target 记录 `snapshot_id + manifest_object_id`）。

#### 3) 导入/恢复必须可重建正确索引

- 在新设备导入配置后：
  - 对每个被选择恢复的 endpoint/targets，能从其所属 chat 下载对应的 latest index 并落盘；
  - 若 bootstrap/catalog 缺失，则该 endpoint 的索引以“空库初始化”开始，后续由首次 backup 逐步建立。

#### 4) 迁移策略（冻结）

- 仅从“下一次备份开始”使用 per-endpoint 本地索引库；不对既有全局 `index.sqlite` 做自动拆分迁移。
- 处理规则：
  - 若发现旧全局 `TELEVYBACKUP_DATA_DIR/index/index.sqlite` 存在：不读取、不写入；当检测到 per-endpoint DB 已可用后自动删除（见下条）。
  - 新的 per-endpoint 索引库缺失时：按“首次备份”路径初始化新 DB 并开始写入。
- 迁移期 UX（frozen）：对旧全局 `index.sqlite` 的存在保持静默（不在 CLI/GUI 中弹提示）。
- 自动清理（frozen）：
  - 当“所有仍在使用的 endpoints”都已具备可用的 per-endpoint 索引库后，自动删除旧全局 `TELEVYBACKUP_DATA_DIR/index/index.sqlite`（保持静默）。
  - “仍在使用的 endpoints”定义（frozen）：满足以下任一条件的 endpoint：
    - 当前 settings 中存在 `enabled=true` 的 target 引用该 endpoint；或
    - 导入/恢复流程中用户选择要恢复该 endpoint 的任意 target。
  - “per-endpoint 索引库可用”最小校验（frozen）：文件存在、可打开、schema 迁移初始化成功（无需包含任何 snapshot 记录）。


## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Local index DB path layout | File format | internal | Modify | ./contracts/file-formats.md | core | CLI/daemon/GUI | 从单库到 endpoint-scope |
| Remote index upload content | Behavior | internal | Modify | ./contracts/file-formats.md | core | CLI/daemon | 按 endpoint 过滤/拆库 |
| Settings validation: unique chat_id | Config | internal | Modify | ./contracts/config.md | core | CLI/GUI | 禁止 chat 复用 |
| `backup run` / index sync（#0012） | CLI | internal | Modify | ./contracts/cli.md | CLI | GUI/daemon | 需要按 endpoint 处理 |

### 契约文档

- [contracts/README.md](./contracts/README.md)
- [contracts/cli.md](./contracts/cli.md)
- [contracts/config.md](./contracts/config.md)
- [contracts/file-formats.md](./contracts/file-formats.md)


## 验收标准（Acceptance Criteria）

- Given 有两个 endpoints（`ep_a`/`ep_b`）且分别绑定到不同的 `chat_id`，
  When 分别对两个 endpoints 各执行一次 backup，
  Then `ep_a` 的 chat 中产生的 remote index 解密后只包含 `provider=telegram.mtproto/ep_a` 的数据，`ep_b` 同理。

- Given Settings 中两个 endpoints 使用了相同 `chat_id`，
  When 执行 `settings set` 或导入 apply，
  Then 返回 `config.invalid` 并提示“chat_id 不可复用”。


## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：
  - 采用“方案 B 拆库”后，backup/restore/verify 路径都要改 db_path 选择逻辑（实现面更大），但模型更清晰且能彻底避免跨 endpoint 重复上传。
- 需要决策的问题：
  - None
- 假设（需主人确认）：
  - None
