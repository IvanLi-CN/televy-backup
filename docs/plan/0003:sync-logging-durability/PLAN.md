# Sync 日志落盘与可排查性（每轮独立日志 + env 配置日志等级）（#0003）

## 状态

- Status: 待实现
- Created: 2026-01-20
- Last: 2026-01-20

## 已冻结决策（Decisions, frozen）

- 粒度：按一次 `backup|restore|verify` 任务生成一份独立日志文件。
- 落盘语义：任务结束前必须 `flush + fsync`（确保日志真正落盘）。
- 日志格式：NDJSON（JSON Lines），便于后续自动分析。
- 日志等级 env：`TELEVYBACKUP_LOG`（优先）→ `RUST_LOG` → 默认（debug）。

## 背景 / 问题陈述

- 当前同步（backup/restore/verify）相关流程缺少“每轮可追溯的落盘日志”，出现“状态看起来不对/失败原因不明/无法复现”时难以定位问题。
- 关键路径（尤其是 daemon 定时触发）对错误结果与上下文的记录不足，缺少可审计的证据链（what/when/why）。
- 在实现阶段需要建立统一日志策略：默认 debug、每轮一份详细日志文件、可用环境变量配置日志等级，并保证正常结束时日志已 `flush + fsync` 到文件。

## 用户与场景（Users & Scenarios）

- 个人用户：定时/手动触发备份；失败后需要快速定位是配置问题、磁盘/权限问题、Telegram 网络问题、索引一致性问题，还是程序 bug。
- 开发阶段：需要对比不同 run 的行为差异，支持“拿到一份日志就能复盘一次同步”的排查方式。

## 目标 / 非目标

### Goals

- 每轮同步任务（backup/restore/verify）自动生成一份**独立的调试级别日志文件**，包含 run 元数据、阶段性进度、错误上下文与统计摘要。
- 通过环境变量配置日志过滤规则（日志等级/targets），开发阶段默认 debug。
- 确保落盘可依赖：同步任务结束（成功/失败/取消）时保证日志写入已 `flush + fsync` 到文件（尽量避免缓冲导致的丢日志）。
- 确保隐私安全：日志不记录任何 secret（Bot token、master key、密文/明文内容）；必要信息以“引用/长度/哈希”等形式呈现。
- 不破坏现有 CLI 输出契约：`televybackup --events` 的 stdout NDJSON、以及 stderr 的 error JSON 不被日志混入。

### Non-goals

- 不做远端日志上报/集中式日志平台（ELK/OTel collector 等）。
- 不在本计划内实现完整的日志检索/过滤 UI（只保证文件落盘与“可打开”）。
- 不在本计划内引入 metrics/traces 的完整观测体系（如需要另立计划）。

## 范围（Scope）

### In scope

- Rust（cli/daemon/core）的日志框架选型与初始化方式（建议：`tracing` + `tracing-subscriber`）。
- 每轮同步日志文件的目录布局、命名约定、输出格式（NDJSON）与落盘策略（`flush + fsync`）。
- 日志等级配置（env vars）与默认值（开发阶段默认 debug）。
- 关键动作的日志点位规范（任务生命周期、phase begin/end、外部 I/O、错误上下文与重试）。
- 最小测试覆盖（不引入新工具）：env filter 解析、log path 生成、关键敏感字段不落盘的断言。

### Out of scope

- 日志保留/清理策略的 UI 化（如需要可另立计划或在实现阶段按默认策略落地）。
- 将“同步状态不准确”的全部根因修复在一个 PR 内（本计划优先补齐排查能力；必要时再拆分后续计划）。

## 需求（Requirements）

### MUST

- 必须为每次同步任务（backup/restore/verify）生成一份独立日志文件，且文件名可唯一定位到一次 run（包含 `kind` 与 `run_id`，并包含可读时间戳）。
- 必须提供环境变量配置日志过滤规则（等级/targets），默认 debug（见 `./contracts/config.md`）。
- 必须确保日志落盘可依赖：
  - 同步任务结束（成功/失败/取消）时，保证已产生的日志已 `flush + fsync` 到对应文件；
  - 进程异常终止场景提供“尽力落盘”（best effort），并在文档中明确其边界与限制（例如 `SIGKILL` 无法保证）。
- 必须保证不污染现有机器可解析输出：
  - `televybackup --events`：stdout 只输出事件 NDJSON；
  - 错误输出：stderr 只输出 error JSON（保持当前 `emit_error` 口径）。
- 必须对关键动作打点并包含可排查字段（最小集）：
  - run.start / run.finish（含：`kind`、`run_id`、`task_id`（如存在）、耗时、结果统计、error_code/message（如失败））
  - 主要 phase（scan/upload/index/restore/verify）开始与结束（含阶段耗时与计数器增量）
  - 外部 I/O（Telegram upload/download、SQLite open/migrate、Keychain read）在失败时必须记录足够上下文（不含 secrets）。
- 每轮同步日志文件内容必须为 NDJSON（每行一个 JSON object；UTF-8；以 `\\n` 分隔），并保证在任务返回后可被读取与逐行解析。

## 接口契约（Interfaces & Contracts）

### 接口清单（Inventory）

| 接口（Name） | 类型（Kind） | 范围（Scope） | 变更（Change） | 契约文档（Contract Doc） | 负责人（Owner） | 使用方（Consumers） | 备注（Notes） |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 日志配置（环境变量） | Config | internal | New | ./contracts/config.md | core/daemon/cli | daemon/cli/gui | 提供日志等级/过滤与日志目录覆盖 |
| 同步日志文件布局与命名 | File format | internal | New | ./contracts/file-formats.md | core/daemon/cli | gui/ops/user | 每轮同步独立文件；保证可定位与可打开 |

### 契约文档（按 Kind 拆分）

- [contracts/README.md](./contracts/README.md)
- [contracts/config.md](./contracts/config.md)
- [contracts/file-formats.md](./contracts/file-formats.md)

## 约束与风险（Constraints & Risks）

### 约束（Constraints）

- CLI 当前通过 stdout 输出人类可读文本或 NDJSON events；stderr 输出 error JSON。日志系统不得破坏该契约。
- 日志中可能出现本地路径、snapshot_id、chunk/hash 等“隐私相关元数据”；需避免泄露 secrets，并给出最小必要信息。

### 风险（Risks）

- Debug 日志可能较大：需要明确默认日志目录与（可选的）保留策略，否则可能造成磁盘占用增长。
- 若采用异步/非阻塞写入，为吞吐引入缓冲：需要明确 `flush/fsync` 时机与“异常退出可能丢最后一段日志”的边界。

## 验收标准（Acceptance Criteria）

- Given 执行 `televybackup backup run --events ...`，
  When 同步任务开始并产生事件输出，
  Then stdout 只包含合法 NDJSON events（不混入日志文本/JSON），且对应的 run 日志文件已在预期目录创建。
- Given 任意一次同步任务（backup/restore/verify），
  When 任务成功结束，
  Then 该 run 日志文件包含 `run.start` 与 `run.finish`（成功态），并包含耗时与结果统计摘要，且任务返回前已完成 `flush + fsync`。
- Given 任意一次同步任务，
  When 任务失败结束（例如配置缺失/Telegram 网络错误/SQLite 错误），
  Then 该 run 日志文件包含 `run.finish`（失败态）以及可排查的 error_code/message（不含 secrets），并在任务返回后可在磁盘上读取到这些日志，且任务返回前已完成 `flush + fsync`。
- Given 未设置任何日志过滤环境变量，
  When 执行一次同步任务，
  Then 日志默认按 debug 输出（以便开发期排查）。
- Given 设置 `TELEVYBACKUP_LOG=info`（或等价规则），
  When 执行一次同步任务，
  Then run 日志中不会出现 debug 级事件（或显著减少到符合过滤规则）。
- Given 任意一次同步任务，
  When 读取该 run 的日志文件并逐行解析 JSON，
  Then 每一行都必须是合法 JSON object（不允许出现混入的非 JSON 文本行）。

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests:
  - env filter 解析与优先级（`TELEVYBACKUP_LOG` vs `RUST_LOG` vs default）。
  - log path 生成（目录/文件名稳定且不含不安全字符）。
  - 敏感字段不落盘（至少覆盖 Bot token 与 master key 的“不会出现在日志文本中”的断言）。
- Integration tests:
  - 在不依赖真实 Telegram 的情况下，跑一次最小同步流程（可用 in-memory / mock storage）并断言生成日志文件且可读。

### Quality checks

- 按仓库既有约定执行 lint/format/test（不引入新工具）。

## 文档更新（Docs to Update）

- `docs/requirements.md`: 补充 Observability 细化（每轮日志文件、默认等级、如何调整等级/定位日志目录）。
- `docs/architecture.md`: 增补 data locations（log dir）与日志输出边界（stdout/stderr 契约）。
- `README.md`: 补充“日志在哪里、如何调日志等级、如何用于排查”的最小指引。

## 实现里程碑（Milestones）

- [ ] M1: 引入 `tracing` 日志基础设施（cli/daemon 初始化 + env filter + file writer）
- [ ] M2: 每轮同步 run 创建独立日志文件（命名/目录/元数据 + `flush+fsync` 策略落地）
- [ ] M3: 在 core 同步流程中补齐关键日志点位（phase begin/end + error context + summary）
- [ ] M4: 测试与文档对齐（tests + docs updates；确保不破坏 CLI 输出契约）

## 方案概述（Approach, high-level）

- 日志框架：使用 `tracing` 作为统一入口，`tracing-subscriber` 负责格式化与过滤；文件写入优先走“独立 run 文件（NDJSON）”，避免污染 stdout/stderr。
- 过滤策略：默认 debug（开发期），但允许通过 env 细粒度控制（例如只把本项目 crate 设为 debug，依赖库设为 info/warn）。
- 落盘策略：同步任务结束时保证 `flush + fsync`；若使用非阻塞写入，需要持有 flush guard 直到任务结束，并补齐 fsync 语义（或直接采用可明确 fsync 的写入策略）。
- 隐私策略：对 secrets 做“禁止落盘”约束；日志字段中只记录必要元数据（不记录明文内容）。

## 风险与开放问题（Risks & Open Questions）

### 开放问题（需要主人决策）

None

## 假设（Assumptions，需要确认）

None

## 参考（References）

- `tracing-subscriber` 的 `EnvFilter`：支持从默认环境变量（`RUST_LOG`）或自定义 env var 解析过滤规则。
- `tracing-appender` 的 `WorkerGuard`：用于在非阻塞写入场景下，在 drop 时 flush 缓冲日志（避免进程退出丢尾部日志）。
