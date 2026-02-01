# Master key 轮换（in-place, resumable）（#4fexy）

## 状态

- Status: 待实现
- Created: 2026-01-31
- Last: 2026-02-01

## 背景 / 问题陈述

- 系统内长期只允许一个 active master key（`televybackup.master_key`）。
- 为了实现“未完成前旧 master key 仍可用”且可断点续跑，轮换期间会在本地 secrets store 暂存一个 **pending master key**（`televybackup.master_key.next`）：它不是 active key，普通 backup/restore/verify 仍只使用 `televybackup.master_key`。
- 在导入 Config Bundle 时，可能出现“bundle TBK1（新 master key）与本机 master key 不一致”，同时本机已配置 targets（正在使用旧 master key 的备份世界）。
- 需求：提供一个 **master key 轮换**流程：
  - 可暂停/继续/取消；
  - 未成功前旧 master key 仍可用（旧世界不被破坏）；
  - 成功后切换到新 master key，并以新 master key 完成一次“全量备份重建”（新世界）。
  - 轮换任务由 daemon 后台执行；在轮换进行期间必须拒绝其它 backup/restore/verify（避免干扰与影响轮换速度；详见 Requirements）。

## 目标 / 非目标

### Goals

- 提供可恢复的 master key 轮换工作流（start/pause/resume/cancel/status），并把状态持久化到本地，支持重启后继续。
- 轮换期间并行存在：
  - active world（旧 master key）：旧世界保持可用且不被破坏（可在轮换结束/取消后继续 backup/restore/verify）。
  - pending world（新 master key）：执行“全量备份重建”直到完成。
- 轮换完成（commit）后：
  - active master key 切换为新 master key；
  - 远端 pinned bootstrap/catalog 切换为新世界；
  - 本地 per-endpoint 索引库切换为新世界。

### Non-goals

- 不做远端对象 GC：取消/失败可能留下“新 key 加密的远端垃圾对象”。
- 不提供“跨设备同时轮换”的一致性协议（仅保证单设备轮换可控）。

## 范围（Scope）

### In scope

- 本地状态机 + 持久化（rotation state）。
- per-endpoint 索引库双写/双轨（old/current + next）与最终切换（依赖 `#r6ceq` 的 per-endpoint DB 方案）。
- 远端 bootstrap/catalog 的“双轨”：轮换期间不改 pinned；新世界 catalog 单独维护，完成后再 pin。
- CLI 接口与事件（供 GUI 调用与展示）。
- secrets store 约定：在轮换期间持久化 pending master key（仅用于轮换；active 仍为旧 key）。

### Out of scope

- 对历史远端数据做密钥重加密（不可行；只能重备份）。

## 需求（Requirements）

### MUST

#### 1) 轮换触发与确认

- 当检测到 master key mismatch 且本机存在 targets 时，必须提示用户进入“重设 master key（轮换）”流程，并在开始前做风险告知与二次确认。

#### 2) 状态机（可暂停/继续/取消）

- Rotation state 必须至少包含状态：
  - `idle` / `staged` / `running` / `paused` / `cancelled` / `completed`
- `staged`（frozen）：轮换请求已被接受，daemon 已写入 rotation state + `master_key.next` 等必要元数据，但“新世界全量备份重建”尚未开始或尚未进入第一个可计量的处理单元；该状态用于支持 daemon 异步启动与断点续跑。
- 支持操作：
  - `start`：`idle` → `staged`（daemon 异步执行；随后进入 `running`）
  - `pause`：`staged|running` → `paused`
  - `resume`：`paused` → `staged`（daemon 异步继续；随后进入 `running`）
  - `cancel`：`staged|running|paused` → `cancelled`（回到旧世界；不触碰旧 key）
- `status` 必须可查询：返回当前状态、进度（每 endpoint/target）、以及下一步建议动作。
- `pause/cancel` 必须可在轮换任务运行时由“另一个进程”触发：实现上通过更新 rotation state 的 `requestedAction` 字段，并由运行中的轮换任务在安全检查点尽快停下（见 `./contracts/file-formats.md`）。

#### 3) 旧 master key 在未完成前保持可用

- 在 `staged|running|paused` 期间：
  - active 仍为旧 master key；
  - pinned bootstrap/catalog 不得切换；
  - 旧 per-endpoint 索引库不得被覆盖（用于回退与轮换结束后继续使用旧世界）。

#### 3.1) 互斥与限流（frozen）

- 轮换任务必须由 daemon 后台执行（GUI/CLI 仅作为“控制面”发起/查询/暂停/取消/提交）。
- 当 rotation state 为 `staged|running|paused`（视为“轮换进行中”）时：
  - 必须拒绝启动新的 backup/restore/verify（CLI 与 daemon entrypoints 一致）；
  - 返回错误 `rotation.in_progress`（retryable=true），并在 message 中提示用户“轮换进行中，请稍后或先 pause/cancel”。

#### 4) 新 master key 全量备份重建

- 对用户选择参与轮换的 targets（默认全选当前 enabled targets）：
  - 必须以新 master key 执行一次完整备份（upload 全量加密对象、生成新 snapshot、上传新 remote index）。
  - 轮换备份写入 **next 索引库**（见下条），不得污染旧索引库。

#### 5) per-endpoint 索引库双轨（依赖 #r6ceq）

- 轮换期间对每个 endpoint 必须维护两份本地索引库：
  - 当前：`TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`（旧世界）
  - next：`TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite.next`（新世界，轮换备份写入）
- commit 时进行原子切换：
  - 旧 `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite` → `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite.bak.rotated.<timestamp>`
  - `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite.next` → `TELEVYBACKUP_DATA_DIR/index/index.<endpoint_id>.sqlite`

#### 6) 远端 bootstrap/catalog 双轨

- 轮换期间：
  - 旧 pinned catalog 保持不变；
  - 新世界 catalog 必须在远端可更新（但不 pin），以便断点续跑：
    - 每次更新新世界 latest 时上传一个新的 catalog 文档，但不调用 `set_pinned_object_id`；
    - 将该“新世界 catalog 的最新 object_id”写入本地 rotation state 的 `pendingCatalogObjectIdByEndpoint[endpoint_id]`。
- commit 时：
  - 将新世界 catalog pin 为最新 root pointer（替换旧 pinned）。

#### 7) 完成切换（commit）

- 仅当所有参与 targets 都完成新世界全量备份后，才允许 commit。
- commit 前必须再次提示关键后果并二次确认：
  - commit 后旧 master key 将被移除，本机将无法再解密旧世界的数据；
  - 建议用户先导出并妥善保管旧 TBK1（作为“回退到旧世界”的唯一手段）。
- commit 后：
  - active master key 切换为新 master key；
  - 清理 rotation state（或保留只读审计信息，不包含 secret）。

### MUST NOT

- 轮换未完成前不得覆盖/删除旧 master key。
- 轮换过程中不得让“普通备份/恢复路径”使用 pending master key（仅轮换任务可用）。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `televybackup secrets rotate-master-key` | CLI | internal | New | ./contracts/cli.md | CLI/daemon | GUI | start/pause/resume/cancel/status |
| rotation state persistence | File format | internal | New | ./contracts/file-formats.md | core | CLI/daemon | 不含明文 secret |
| `task.progress.phase` 增量 | Event | internal | Modify | ./contracts/events.md | core/CLI | GUI | 新增 `key_rotation` |
| secrets store keys（rotation） | Config | internal | New | ./contracts/config.md | core | CLI/daemon | `master_key.next` 等 |

### 契约文档

- [contracts/README.md](./contracts/README.md)
- [contracts/cli.md](./contracts/cli.md)
- [contracts/config.md](./contracts/config.md)
- [contracts/events.md](./contracts/events.md)
- [contracts/file-formats.md](./contracts/file-formats.md)

## 验收标准（Acceptance Criteria）

- Given 本机已有 targets 且 master key mismatch，
  When 用户发起轮换并完成二次确认，
  Then 轮换进入 running，且对外拒绝启动其它 backup/restore/verify（返回 `rotation.in_progress`，retryable=true）。

- Given 轮换进行中，
  When 用户 pause → resume，
  Then 轮换进度可持续推进且可在应用/daemon 重启后恢复。

- Given 轮换进行中，
  When 用户 cancel，
  Then 轮换停止，旧世界保持可用，新世界不被 pin，active master key 不变。

- Given 轮换完成所有 targets 的新世界全量备份，
  When commit，
  Then active master key 切换为新 key，pinned catalog 切换为新世界，本地索引库原子切换为新世界。

## 实现前置条件（Definition of Ready / Preconditions）

- 已冻结 per-endpoint 索引库布局（`#r6ceq`：Option B）。
- 已确定 rotation state 的持久化位置与形状（见 `./contracts/file-formats.md`）。
- 已确定二次确认的具体交互：typed phrase（输入 `ROTATE`），覆盖 start/commit（cancel/pause 不需要二次确认）。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit：状态机转移；rotation state 持久化；双轨索引库切换的原子性
- Integration：断点续跑（kill/restart）；pause/resume/cancel；commit 切换正确

## 文档更新（Docs to Update）

- `docs/architecture.md`：补充 master key rotation 的双轨模型与数据路径
- `docs/plan/fn4ny:config-bundle-export-import/PLAN.md`：导入时触发 rotation 的流程链接

## 实现里程碑（Milestones）

- [ ] M1: Rotation state 规格 + 持久化
- [ ] M2: CLI 接口（start/pause/resume/cancel/status）
- [ ] M3: 双轨索引库（next 写入 + commit 原子切换）
- [ ] M4: 远端 catalog 双轨（un-pinned 更新 + commit pin）
- [ ] M5: 测试与文档同步
