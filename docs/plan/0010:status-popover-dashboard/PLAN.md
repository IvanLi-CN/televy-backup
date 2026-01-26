# 状态弹窗重做：全局网络 + 多目标面板 + 开发者视图（#0010）

## 状态

- Status: 已完成
- Created: 2026-01-24
- Last: 2026-01-26

## 已确认决策（Decisions, frozen）

- “实时上下行/累计流量”采用**业务层口径**（bytesUploaded/bytesDownloaded），不使用传输层（MTProto session bytes sent/recv）。
- Dev 视图对所有用户可见（不做隐藏开关/手势）。
- Popover 尺寸：宽度固定 `360`；高度按内容自适应，最大高度为 `720`（高宽比 `2:1` 的上限）。当 targets 列表溢出时，列表区域滚动承载长列表，header/global 保持可见（或等价可用性设计）。
- Targets 行仅展示**上行**（业务口径 bytesUploaded）与 last run 概要；不展示 per-target 下行字段（避免在 backup 场景下误导）。last run 次行默认展示 `bytesUploaded`；当 `bytesUploaded=0` 且 `bytesDeduped>0` 时，展示 `saved bytesDeduped`（可附带 `filesIndexed`）。
- 本计划不引入 socket IPC：状态“真源”采用 daemon 落盘快照文件（`status.json`）；UI 默认通过 `televybackup status stream` 获取实时快照；当 CLI 不可用时允许退化为低频轮询 `status.json`（用于本地开发/应急）。

## 背景 / 问题陈述

- 现有菜单栏 Popover 的状态页（Overview）仍按“单目标 + 单次任务”的信息结构组织，无法覆盖“多个备份目标并行存在”的核心观察需求。
- 现状也缺少“全局 network（业务口径 up/down）+ 多 target 状态快照”的稳定数据源与 UI 投影，导致需要在 UI 上展示的关键字段要么缺失、要么只能回退为 `0/—`。
- 需要将 Popover 重做为“实时面板”：一眼看到全局实时上下行、以及每个 backup target 的实时状态/进度/体积信息；并提供一个开发者视图用于完整状态排查。

## 用户与场景（Users & Scenarios）

- 普通用户：打开 Popover 快速确认“是否在跑 / 跑哪个目标 / 大概进度 / 最近一次结果”。
- 维护者/开发者：打开独立 Developer window 快速定位“数据源是否更新、某个 target 状态为何不动、是否存在 stale/错误码、底层字段取值、近期活动”。

## 目标 / 非目标

### Goals

- Popover 重做为单视图结构（无 tabs）：聚焦“Overview 面板”。
- Overview 必须包含两块核心信息：
  - **全局**：实时上下行速率 + 自 UI 启动后累计上下行流量（session totals）。
  - **每个 backup target**：实时上行、体积、状态（最近一次运行时间/用时；运行中显示进度/已用时）。
- Dev 视图尽量充分：提供独立 Developer window，展示全局状态与每个 target 的详细状态（包含原始字段、时间戳、数据来源、错误字段等），并提供 activity 时间线，便于排障。
- 数据更新“无明显滞后”：UI 端在运行态对关键字段的可感知延迟应受控（见 NFR 与验收）。
- 明确定义显示逻辑：单位切换、舍入、异常/缺失值、stale 标记、颜色语义。
- 产出 UI 设计图（Overview/Dev）+ 设计说明文档（冻结口径）并可被实现直接落地。

### Non-goals

- 不在本计划内引入复杂多窗口 UI（Popover 仍是主要承载；Settings window 不在此处重做）。
- 不在本计划内新增“日志浏览 UI”（排查信息以 Dev 视图与既有日志落盘路径为主）。
- 不在本计划内重新定义 backup pipeline 的业务语义（仅做状态/可观测性投影与 UI 组织）。

## 范围（Scope）

### In scope

- Popover 尺寸与布局：宽度固定 `360`；高度按内容自适应，最大高度为 `720`。targets 列表溢出时使用滚动，且提供滚动边缘渐隐提示（见 NFR）。
- Overview 信息架构与视觉基准图（SVG source + PNG preview）。
- Dev 信息架构与视觉基准图（SVG source + PNG preview）。
- 显示逻辑与状态机口径：单位/舍入/时间格式、stale、错误、运行中进度与 ETA（若可）。
- 定义“状态数据的接口契约”（CLI / Events / File formats），确保实现可测试、可持续引用。

### Out of scope

- 任何实现代码、配置、依赖变更（本计划阶段只冻结口径；实现另立阶段）。
- Settings window 的信息架构改造（若 Dev 视图需要跳转/入口，仅定义入口，不在此处重做 Settings UI）。

## 需求（Requirements）

### MUST

- Popover 不提供 tabs/segmented 导航；打开即为 Overview。
- Popover Header 必须提供 `Backup now`（立即备份）按钮：
  - 可直接点击触发一次立即备份（不依赖打开 Settings）。
  - 多 targets 时的默认触发策略需冻结（见“开放问题”）。
- Popover（Overview）：全局区必须展示：
  - `Up`/`Down` 实时速率（单位自动切换；业务口径）。
  - `Up`/`Down` 自 UI 启动以来累计值（session totals；业务口径）。
  - `Last updated`（或等价的 freshness 提示），用于判断是否 stale。
- Popover（Overview）：targets 列表必须展示每个 target：
  - 标识：`label`（若空则回退到短 `target_id`）+ `source_path`（可截断）。
  - 运行状态：`idle/running/failed/stale`（成功通过 `lastRun.status=succeeded` 表示；视觉语义一致）。
  - 实时上行速率（单位自动切换；业务口径）。
  - “文件体积”（见 Display Logic 定义）：至少包含 `bytesRead` 或 `bytesUploaded` 的一个主指标；若两者可得则同时展示（一个主、一个次）。
  - 最近一次运行：时间点 + 用时（若可得）；失败时展示错误码（短）。
  - 运行中：显示进度条（可得则 determinate，否则 indeterminate）+ 已用时。
- Popover（Overview）：当 `targets=0`（未配置/为空）：
  - 展示 empty state 引导文案，说明需要在 Settings 添加 targets。
  - 提供主按钮 `Open Settings…`（打开 Settings window；仅提供入口，不新增 Settings 页面）。
- Dev：必须展示全局与 per-target 的“原始字段”与关键时间戳：
  - 数据源/路径、schema version、生成时间、UI 接收时间、stale 判定依据
  - per-target：状态机字段、progress 原始计数（files/chunks/bytes）、最近一次结果与错误码、endpoint 绑定信息
- Dev：必须包含 Activity（工作活动时间线）：
  - 至少包含：快照更新、落盘写入、phase/进度推进、错误出现、stale 触发
  - 每条必须有时间戳（精确到毫秒或等价精度）
- UI 更新延迟：运行中状态更新频率目标 `≥ 5Hz`（200ms 内可感知更新），静止态可降频（见 NFR）。
- 缺失字段必须有稳定的回退显示（`—`/`unknown`），不得出现闪烁式跳变。

## 显示逻辑（Display Logic, frozen）

### 1) 单位与舍入（bytes / bytes-per-second）

- 基数：`1024`（KiB/MiB/GiB），但 UI 文案使用 `KB/MB/GB`（与现有 `formatBytes` 保持一致），并在文档中明确“基于 1024”。
- `bytes`：
  - `< 1024`：整数 `B`；
  - `KB` 以上：默认 1 位小数；
  - 当 `value ≥ 100`（当前单位）时：改为 0 位小数（减少抖动）。
- `B/s`（速率）：
  - `< 10 KB/s`：保留 1 位小数（提高可读性）；
  - `≥ 10 KB/s`：1 位小数；
  - `≥ 100 MB/s`：0 位小数。
- “抖动控制”：速率展示采用滑动窗口均值（默认 1.0s 窗口）+ 限制最小显示步进（例如 0.1 MB/s）。
- 语义（业务口径）：
  - Global `Up` = `bytesUploaded` 的变化率（upload to remote）。
  - Global `Down` = `bytesDownloaded` 的变化率（download from remote）。
  - Target `Up` = `bytesUploaded` 的变化率（upload to remote）。

### 2) 时间（last run / elapsed）

- `elapsed`：`< 60s` 显示 `Ss`；`< 1h` 显示 `MmSSs`；`≥ 1h` 显示 `HhMm`。
- `last run`：Popover 内优先相对时间（`just now / 12m ago / 2h ago / 3d ago`）；Dev 视图展示 ISO8601（含时区）。

### 3) 进度条（progress bar）

- Determinate 优先级：
  1) `chunksDone/chunksTotal`
  2) `filesDone/filesTotal`
  3) 若均缺失：indeterminate（并在副标题展示可得的 bytes 指标）。
- 进度条旁必须显示“已用时”；若可以估算 ETA，则显示 `ETA`（可选，但需稳定，不抖动）。

### 4) stale 判定

- 若 `now - snapshot.generatedAt > 2s`：在 Overview 顶部显示 `Stale`（弱红/橙），targets 列表各行也显示 stale icon。
- 若超过 `10s`：视为“断开/不可用”，targets 的实时字段显示 `—`，并给出指引（Dev 视图保留最后一份快照供排障）。

### 5) Popover 高度自适应（Layout sizing）

- 宽度固定：`360`。
- 高度自适应目标：
  - 当 targets 较少且列表未溢出：Popover 高度应收敛到“刚好容纳内容”的高度（不强行撑到最大高度）。
  - 当 targets 较多导致列表溢出：Popover 高度达到上限 `720`，targets 列表区域滚动承载剩余内容（header/network/targets 标题保持可见或等价可用性设计）。
- 建议实现口径（可测试的计算规则）：
  - `height = min(maxHeight, chromeHeight + listHeight)`，其中 `maxHeight=720`，`chromeHeight` 为 header + network + section 标题等固定区域高度；
  - `listHeight = min(listMaxHeight, listContentHeight + contentInsetTop + contentInsetBottom)`；
  - 当 `targets=0` 使用 empty state（见设计图），Popover 高度随 empty state 内容自然收敛。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `televybackup status get` | CLI | internal | New | ./contracts/cli.md | Rust CLI | macOS UI | 返回单次 `StatusSnapshot` |
| `televybackup status stream` | CLI | internal | New | ./contracts/cli.md | Rust CLI | macOS UI | NDJSON 连续输出快照（用于低延迟） |
| `status.snapshot` 事件 | Event | internal | New | ./contracts/events.md | Rust CLI/daemon | macOS UI | 每条为完整快照，便于 UI 直接渲染 |
| `status.json` | File format | internal | New | ./contracts/file-formats.md | daemon | CLI | daemon 以原子写落盘；CLI 读取并对 UI 输出 NDJSON |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/cli.md](./contracts/cli.md)
- [contracts/events.md](./contracts/events.md)
- [contracts/file-formats.md](./contracts/file-formats.md)

## 验收标准（Acceptance Criteria）

- Given 用户点击菜单栏图标打开 Popover，
  When 查看弹窗内容，
  Then 能看到全局 `Up/Down` 实时速率与 session totals，且每个 target 都有一行状态展示（targets 行仅展示 `Up`；缺失字段以 `—` 稳定回退）。
- Given 用户点击 Header 的 `Backup now`（立即备份），
  When 触发成功，
  Then UI 能在 targets 列表中观察到对应 target 进入 `Running` 并开始更新进度（或给出明确失败提示与可排障入口）。
- Given 任一 target 正在运行且底层有持续进度更新，
  When 用户观察 Overview，
  Then 进度条与关键数字以 `≥ 5Hz` 频率更新且无明显卡顿/滞后（人眼可感知延迟 ≤ 200ms 目标）。
- Given 用户在 Settings window 点击 `Developer…` 打开独立 Developer window，
  When 查看全局与 per-target 字段，
  Then 能定位快照生成时间/数据源、每个 target 的原始字段（含 progress 计数与错误码），用于排障不依赖外部日志。
- Given 底层状态源停止更新（模拟 stale），
  When 用户打开 Popover，
  Then UI 明确标记 `Stale`，并避免展示误导性的“仍在跑”的动态数值。
- Given 存在多个 targets，
  When targets 数量超过 Overview 可视区，
  Then targets 区域可滚动，且 header/global 区保持可见（或等价的可用性设计）。

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认“全局上下行速率/累计流量”的数据口径（业务 bytesUploaded/bytesDownloaded）并冻结。
- 已确认 targets 行不展示 per-target 下行字段（仅上行）。
- 已确认状态数据源：daemon 落盘 `status.json`（路径/原子写/刷新频率）→ CLI `status stream`（输出 NDJSON），以及刷新频率上限。
- 已确认 Dev 视图入口对所有用户可见。
- Repo reconnaissance（已完成，供实现落点定位）：
  - `macos/TelevyBackupApp/TelevyBackupApp.swift`：`PopoverRootView` 当前为 header + `OverviewView()`（无 tabs/Logs）；本计划主要替换 Overview 为“全局 network + 多 target 列表”。
  - `macos/TelevyBackupApp/TelevyBackupApp.swift`：当前 `popover.contentSize = NSSize(width: 360, height: 460)`；本计划要求“宽 360、高度自适应（max 720）”，实现需按 targets 内容动态调整 content size（溢出时列表滚动）。
  - `macos/TelevyBackupApp/TelevyBackupApp.swift`：Settings window 已通过 header gear 打开；本计划要求在 Settings 内“增加 Developer… 入口”（不新增 Settings 页面），点击打开独立 Developer window。

## 非功能性验收 / 质量门槛（Quality Gates）

### Performance

- 运行中 UI 更新目标：`≥ 5Hz`；静止态可降至 `1Hz`（以不显著增加功耗为准）。
- 不允许通过高频启动短命 CLI 进程实现实时（避免 CPU/电量与抖动）；实时路径应为“单一长连接/长进程 stream”或等价机制。
- daemon 侧状态快照（`status.json`）写入需要限频（建议上限 10Hz）并保证单条快照体积可控（避免 IO/CPU 抖动）。
- Scroll UX：targets 列表在可滚动时，顶部/底部应提供渐隐提示；实现应使用“内容 alpha mask”（而非覆盖一层带颜色的遮罩），以兼容 popover 半透明材质。
- 渐隐显示规则：仅当列表可滚动时启用；未到顶部时显示顶部渐隐、未到底部时显示底部渐隐（到顶/到底关闭对应边）。
- Layout UX：Popover 高度需随内容自适应；当 targets 数量较少时，Popover 不应强制拉到最大高度；当 targets 溢出时，高度达到上限并启用列表滚动（不得出现内容贴边或溢出圆角）。
- List Insets：targets 列表需要明确 `contentInsetTop/contentInsetBottom`（推荐 bottom≥16px），确保首/尾行在滚动边缘不会贴边或被圆角裁切。

### Testing

- Contract tests（Rust）：对 `StatusSnapshot` schema（序列化/字段缺失容错）与 `status stream` NDJSON 格式做单测。
- UI smoke（macOS）：验证 Overview/Dev 的刷新频率、stale 提示、单位/舍入规则一致性。

## 文档更新（Docs to Update）

- `docs/design/ui/statusbar-popover-ia.md`: 更新 Popover IA（移除 tabs/Logs；Overview 变更为“全局 network + 多 target 列表”；Dev 为独立窗口且入口在 Settings）。
- `docs/design/ui/README.md`: 增加本计划设计图入口与预览指引。

## 实现里程碑（Milestones）

- [x] M1: 定义并实现状态数据源（`status get/stream` + `StatusSnapshot`）
- [x] M2: Popover Overview 重做（全局网络 + 多 target 列表 + 进度/状态）
- [x] M3: Popover Dev 视图落地（全局 + per-target 原始字段展示）
- [x] M4: 测试与文档更新（契约测试 + UI smoke + IA 文档）

## 设计图与说明（Design assets）

- `docs/design/ui/statusbar-popover-dashboard/popover-overview.svg` / `docs/design/ui/statusbar-popover-dashboard/popover-overview.png`
- `docs/design/ui/statusbar-popover-dashboard/popover-overview-empty.svg` / `docs/design/ui/statusbar-popover-dashboard/popover-overview-empty.png`
- `docs/design/ui/statusbar-popover-dashboard/developer-window.svg` / `docs/design/ui/statusbar-popover-dashboard/developer-window.png`
- `docs/design/ui/statusbar-popover-dashboard/README.md`
- `docs/design/ui/statusbar-popover-dashboard/_preview-popover.html`

## 方案概述（Approach, high-level）

- UI 侧以“单一状态快照（StatusSnapshot）”渲染：Overview 与 Dev 均只依赖同一份快照（避免两套口径）。
- 数据侧采用 `status stream`：UI 启动一个长生命周期进程读取 NDJSON，按快照驱动渲染，避免轮询/抖动/电量开销。
- `status stream` 的快照来源为 daemon 落盘 `status.json`（见 `./contracts/file-formats.md`）；CLI 负责读取并输出统一的 NDJSON `status.snapshot`。
- 对“实时速率”采用滑动窗口计算：后端或 UI 任选其一，但必须保证稳定性与可测试（契约中明确）。

## 风险 / 开放问题 / 假设（Risks, Open Questions, Assumptions）

- 风险：现有后端 `TaskProgress` 字段不包含网络层 tx/rx；需要新增埋点与汇聚后才能满足全局上下行需求。
- 风险：daemon 与 UI 进程的生命周期与数据源耦合不清晰时，可能导致 stale/误报；需在契约中引入 `generatedAt` 与 `source` 字段。
- 假设（需主人确认）：Dev 视图默认对所有用户可见（不做隐藏手势/开关）；若需要隐藏，将在实现前置条件中补充开关策略。
- 开放问题（需主人确认）：多 targets 场景下点击 `Backup now` 的默认策略：
- ✅ A) 立即备份所有 enabled targets（顺序/并发按既有后端策略）

## Change log

- 2026-01-25：实现 `status.json`（daemon）+ `televybackup status get/stream`（CLI）+ Popover Overview（全局 network + targets）+ Developer window（原始字段 + activity + Copy JSON/Reveal/Freeze）；同步设计资产到 `docs/design/ui/` 并更新 IA 文档；验证：`cargo test`、`scripts/macos/build-app.sh`。
- 2026-01-25：Popover 打开时 best-effort 拉起 `televybackupd`：优先 `launchctl kickstart gui/<uid>/homebrew.mxcl.televybackupd`，无服务时回退为从 app bundle（或 PATH）直接启动；`scripts/macos/build-app.sh` 将 `televybackupd` 打进 `.app`，确保本地构建可自动拉起。
- 2026-01-25：对齐 Popover Overview 视觉基准图：NETWORK/updated 排版、Up/Down chip 样式、Targets list（badge/row/empty state）与滚动分隔线；并将 daemon/status stream 的 best-effort 启动前置到 app launch（无需先打开 popover）。
- 2026-01-26：订正设计基准图：Targets 行 `label`↔badge 间距统一（视觉约 10px）；右侧信息语义固定为“主行时间类 + 次行数值类”，避免不同状态下右侧含义乱跳，并同步到 IA 文档。
- 2026-01-26：新增 `Backup now`（立即备份）按钮：多 targets 策略冻结为“立即备份所有 enabled targets”；实现为 UI 写入 `$TELEVYBACKUP_DATA_DIR/control/backup-now`，daemon 轮询消费触发并执行备份。
- 2026-01-26：补齐“短任务可见性”：`lastRun` 增加 `filesIndexed`；Popover idle 次行在 `bytesUploaded=0` 但 `bytesDeduped>0` 时展示 `saved bytesDeduped`（可附带 files）；并在观测到新 `lastRun` 时弹 toast 提示完成/失败。修复 UI 启动 CLI 时 env 不一致问题（传递 `TELEVYBACKUP_CONFIG_DIR`/`TELEVYBACKUP_DATA_DIR`）；当 CLI 不可用时退化为低频轮询 `status.json`，避免面板空白/误判断开。
