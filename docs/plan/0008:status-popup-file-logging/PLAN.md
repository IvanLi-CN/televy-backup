# 状态弹出界面移除日志页（日志仅落盘）（#0008）

## 状态

- Status: 待实现
- Created: 2026-01-22
- Last: 2026-01-23

## 已确认决策（Decisions, frozen）

- “状态弹出的界面”指 macOS 菜单栏 Popover（当前 `Overview / Logs / Settings`）。
- 移除范围包含 UI 内存日志缓存：不再维护 UI 内存中的日志列表；日志仅写入文件用于排查。
- 本计划仅涉及 UI 日志（`ui.log`）；后端每轮同步日志（NDJSON）继续按既有规则落盘（见 References）。
- UI 日志文件位置采用“对齐后端日志目录”的策略（见 `./contracts/file-formats.md`）。
- Settings 的 “Open logs” 仅提供 1 个入口，打开日志目录（覆盖 per-run logs + `ui.log`）。

## 背景 / 问题陈述

- macOS App 的菜单栏 Popover（状态弹出界面）目前包含 `Overview / Logs / Settings` 三个 Tab，其中 `Logs` 展示一段 UI 内存日志列表。
- Popover 的主要用途是“快速看状态/触发动作”，日志列表属于排查用途，常驻在 Popover 会增加界面复杂度且信息噪声较高。
- 希望将日志排查回归到“落盘文件”路径：Popover 不再展示日志界面，但日志仍可持续写入文件，便于离线排查与复现。

## 用户与场景（Users & Scenarios）

**用户**

- 单用户 macOS：通过菜单栏 Popover 快速查看备份状态、进入设置、触发刷新/测试连接等操作。

**核心场景**

- 用户打开 Popover，仅查看状态（是否在线 / 是否在跑 / 最近一次运行结果）并进行少量操作（Refresh / 进入 Settings）。
- 当出现异常（连接失败/命令失败）时，用户/维护者通过日志文件定位问题，而不是在 Popover 里翻日志列表。

## 目标 / 非目标

### Goals

- Popover 移除 `Logs` 界面：不再提供 `Logs` Tab 与日志列表视图。
- 移除 UI 内存日志缓存：不再在 UI 内维护日志列表。
- 增加日志入口：在 Settings 提供“打开日志目录”的入口（单一入口），作为替代排查路径。
- 日志必须落盘：Popover 不展示日志并不等于“没有日志”；需要明确并保证日志写入文件的口径（位置 + 格式 + 基本隐私保护）。
- 文档可查：在项目文档中明确“去哪找日志文件”。

### Non-goals

- 不在本计划内实现新的日志 UI（例如“打开日志文件/目录”的弹窗或内置 viewer）。
- 不改变 sync 的 per-run NDJSON 日志契约（见计划 #0003），也不引入新的日志采集/上传能力。
- 不引入日志轮转/压缩/保留策略（如需要，另立计划）。

## 范围（Scope）

### In scope

- macOS Popover：移除 `Logs` 界面（导航与布局调整以不再出现日志浏览为准）。
- 明确并固化“UI 日志落盘”口径（文件路径、编码、行格式、脱敏规则），并补齐对应契约文档。
- 文档更新：补充“日志位置”说明，便于用户自行排查。

### Out of scope

- 对 `scripts/macos/build-app.sh`、Rust 核心、daemon 的行为做任何额外重构（除非实现阶段发现必须的最小改动）。

## 需求（Requirements）

### MUST

- Popover 不再出现 `Logs` Tab，也不再出现日志列表界面。
- 移除 UI 内存日志缓存：不再在 UI 内维护 `logEntries` 列表（避免“日志仍存在但不展示”的半吊子状态）。
- UI 产生的日志（用于排查 UI/命令调用错误）必须写入本地文件，并且：
  - 追加写（append-only），不覆盖既有内容；
  - 采用 UTF-8 文本；
  - 每行包含明确时间戳（避免时区歧义）。
- 日志不得泄露敏感信息（至少需要维持现有对 `api.telegram.org` 相关片段的脱敏规则）。
- 文件落盘失败不得导致 UI 崩溃或递归写日志（best effort）。
- Settings 增加一个入口：可一键打开“日志目录”（Finder），覆盖 per-run logs 与 `ui.log`。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| UI 日志文件：`ui.log`（见契约） | File format | external | Modify | ./contracts/file-formats.md | macOS app | 用户/维护者 | Popover 移除日志 UI 后，日志排查路径以该文件为准 |
| Popover：移除 Logs | UI Component | external | Modify | ./contracts/ui-components.md | macOS app | 用户 | 不再提供日志浏览 UI（其余导航结构可能被计划 #0005 改造） |
| Settings：Open logs | UI Component | external | Modify | ./contracts/ui-components.md | macOS app | 用户 | 打开日志位置（Finder） |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/file-formats.md](./contracts/file-formats.md)
- [contracts/ui-components.md](./contracts/ui-components.md)

## 验收标准（Acceptance Criteria）

- Given 用户打开菜单栏 Popover，
  When 查看导航 Tab，
  Then 不出现 `Logs`（不提供日志列表/日志浏览界面）。
- Given 用户打开菜单栏 Popover，
  When 反复刷新/触发错误，
  Then UI 不会产生内存日志列表（无 `Logs` UI、无“隐藏的 log list”）。
- Given UI 触发任意会写日志的动作（例如启动、Refresh、命令失败），
  When 在日志目录下检查 `ui.log`，
  Then 文件存在且新增行被追加写入，格式符合契约定义。
- Given 日志内容包含 `api.telegram.org` 相关片段，
  When 写入 `ui.log`，
  Then 该片段会被脱敏（不出现原始 URL 片段）。
- Given 用户打开 Settings，
  When 点击 “Open logs”，
  Then 系统打开日志目录（Finder），用户可看到 per-run logs 与 `ui.log`。

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认“状态弹出界面”的具体指代（Popover vs 其他状态弹窗），以及移除范围。
- 已确认 UI 日志文件的路径与是否需要对齐 `TELEVYBACKUP_*_DIR` 环境变量口径（见契约）。
- 已确认 “Open logs” 打开日志目录（单一入口）。
- 验收标准覆盖 core path + 日志落盘失败场景（best effort）。
- 关键文件与入口点已定位（见下方 References）。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Manual smoke (macOS): 运行 `scripts/macos/build-app.sh` 构建并启动 App，验证 Popover UI 与 `ui.log` 落盘。
- Regression: 不影响既有 Rust CI（`cargo fmt --check` / `cargo clippy` / `cargo test`）。

### Quality checks

- 按仓库既有约定执行 Rust 侧质量检查（不引入新工具）。

## 文档更新（Docs to Update）

- `README.md`: 在 troubleshooting/日志段落补充 UI 日志文件 `ui.log` 的默认位置与用途（区分 per-run NDJSON 与 UI 日志）。
- `docs/architecture.md`: 补充“可观测性/日志”小节对 UI 日志位置的引用（如该文档已有相关章节，则只追加最小说明）。
- `docs/plan/0003:sync-logging-durability/contracts/file-formats.md`: 补充一句“UI 日志文件 `ui.log` 与 per-run logs 同目录”的说明（保持边界清晰）。

## 实现里程碑（Milestones）

- [ ] M1: 移除 Popover 的 `Logs` Tab（导航与布局对齐，确保无悬挂入口）
- [ ] M2: 固化并验证 UI 日志落盘（路径/格式/脱敏/失败 best effort）与契约一致
- [ ] M3: 更新文档（`README.md` / `docs/architecture.md`）说明日志位置与排查路径

## 方案概述（Approach, high-level）

- UI：删去 `Tab.logs` 与对应视图入口，Popover 只保留“状态概览 + 设置”。
- 可观测性：把“排查入口”迁移到落盘文件；通过契约文档固定日志文件的路径与格式，并在 Settings 提供“打开日志目录”入口，避免实现阶段口径漂移。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：移除 `Logs` 后，若没有替代入口（例如文档/打开路径指引），排查体验可能下降；需用文档补齐。
- 风险：UI 日志文件路径目前可能未对齐 `TELEVYBACKUP_CONFIG_DIR` / `TELEVYBACKUP_DATA_DIR` 口径，若需要一致性会涉及兼容策略。
- 风险：计划 #0005 已涉及 Popover 导航重构（移除 Settings tab 等）；若保持独立计划，可能在实现阶段产生冲突与合并成本。
- 假设：Popover 的 “Logs” 仅用于调试 UI/命令调用，不作为核心功能入口（移除不会影响正常使用）。

## 参考（References）

- 入口文件：`macos/TelevyBackupApp/TelevyBackupApp.swift`
  - Popover tabs：`SegmentedTabs` / `PopoverRootView`
  - 日志 UI：`LogsView`
  - UI 日志落盘：`uiLogFileURL()` / `appendFileLog(_:)`
- macOS 构建脚本：`scripts/macos/build-app.sh`
- 后端每轮同步日志（NDJSON）契约：`docs/plan/0003:sync-logging-durability/contracts/file-formats.md`
