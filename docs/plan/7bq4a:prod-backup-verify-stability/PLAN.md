# 修复发行版最近备份/验证失败（Telegram 超时、索引误判、瞬态文件、Vault Key 缓存）（#7bq4a）

## 状态

- Status: 已完成
- Created: 2026-02-19
- Last: 2026-02-19
- Notes: PR #44

## 背景 / 问题陈述

近期发行版在执行 backup/verify 时出现多次失败，典型症状包括：

- Telegram MTProto 上传/下载不稳定导致超时、-503、FloodWait 等暂时性错误。
- 远端索引下载失败被误判为“缺失分片（不可重试）”，导致错误码与可重试语义错误，误导用户采取“重新备份”等错误操作。
- 扫描阶段遇到瞬态文件（例如 `.tmp` 文件在遍历窗口被删除）直接失败，降低备份成功率。
- daemon 侧 Vault key 缓存不可失效，导致 secrets 变更后仍返回旧 key，触发偶发 `crypto error` 且难以自愈。

目标是根治上述问题：对暂时性失败自动自愈/重试；对永久性失败给出准确、可诊断且语义正确的错误（尤其是 retryable 分类），不降低既有安全性与功能语义。

## 目标 / 非目标

### Goals

- 远端索引下载错误分类正确：Telegram 暂时性错误保持 `telegram.*`（retryable=true），仅永久缺失才报 `index.part_missing`。
- 扫描阶段对瞬态 `NotFound` 具备容错能力，不因临时文件消失导致整轮失败；权限/真实 I/O 错误仍 fail fast。
- backup 阶段不再出现 `sqlite pool timed out` 掩盖真实上传错误；并保证 remote index 一定包含最新 `chunk_objects` 映射。
- MTProto helper 对下载与 `send_message` 具备 bounded retry（含 FloodWait 支持），并可利用 endpoint rate_limit 驱动节流与并发，提升稳定性。
- Vault key 缓存可失效，并在 secrets crypto 失败时自动恢复（在用户确实没有权限/正确 key 时仍返回明确错误）。

### Non-goals

- 不引入代理/翻墙能力与相关配置项（SOCKS/HTTP proxy）。
- 不改变 Telegram endpoint/targets 的业务语义与现有配置格式（仅增加向后兼容的可选字段）。
- 不降低安全性：不缓存明文 secrets、不放宽加密校验。

## 范围（Scope）

### In scope

- `crates/core`：
  - 修复远端索引下载错误误判（`remote_index_db`）。
  - 扫描阶段忽略瞬态 `NotFound`（`backup`）。
  - collect 阶段改为内存缓冲 + 批量写入 `chunk_objects`，避免 sqlite pool 超时并保证索引一致性（`backup`）。
  - `telegram_mtproto`：避免在 Tokio runtime worker 上长时间阻塞（`block_in_place`）。
  - 扩展 helper init 协议与 core 内部 config（可选字段）以承载 endpoint rate_limit（向后兼容）。
- `crates/mtproto-helper`：
  - 下载分块失败自动重试 + 断点续传（bounded retry）。
  - FloodWait 解析增强（兼容 `(value: N)` 格式）与相关单测。
  - `send_message` bounded retry（含 FloodWait sleep）。
  - 按 init 传入的 `min_delay_ms` 与 `max_concurrent_uploads` 实装节流与并发上限。
- `crates/daemon`：
  - Vault key 缓存可失效；在 secrets crypto 失败时清缓存并触发重载（带护栏避免刷 Keychain）。

### Out of scope

- 大规模重构错误类型体系或 UI 展示逻辑（除非为修复语义错误所必需）。
- 变更 pack/chunking 策略或数据格式（除非为修复一致性所必需）。

## 验收标准（Acceptance Criteria）

- Verify：Telegram 抖动/超时应保持 `telegram.unavailable`（retryable=true），不再被误报为 `index.part_missing`。
- Backup（Projects）：不再以 `sqlite error: pool timed out while waiting for an open connection` 作为最终失败原因；当 Telegram 真实失败时，最终错误码为 `telegram.unavailable`。
- Backup（Sync）：遇到 `.syncthing*.tmp` 等瞬态 `NotFound` 不再失败。
- Vault：当 `secrets.enc` 发生变化导致旧 vault key 失效时，daemon 能自动失效缓存并恢复；CLI verify 不再出现“立刻 crypto error”的死锁状态（除非用户确实无权限/无正确 key）。
- 质量门槛：`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --all-features` 全绿；并额外跑 `cd crates/mtproto-helper && cargo test`（helper crate 不在 workspace 内）。

## Testing

- 单元测试：
  - error 分类（索引下载：暂时性 Telegram vs 永久缺失）。
  - FloodWait 解析新格式。
- 集成/最小验证：
  - `cargo test --all-features`
  - `cd crates/mtproto-helper && cargo test`

## Milestones

- [x] core：远端索引下载错误分类修复 + 测试覆盖
- [x] core：扫描阶段容错（忽略瞬态 NotFound）+ 单测（非 flaky）
- [x] core：backup collect 批量写入 `chunk_objects`，避免 sqlite pool 超时 + 测试覆盖
- [x] helper：下载 retry+断点续传、FloodWait 解析增强、send_message 重试 + 单测
- [x] core+helper：helper init 协议与 config 扩展（`min_delay_ms`/`max_concurrent_uploads`）并打通 daemon/cli 创建点
- [x] daemon：Vault key 缓存可失效 + crypto 失败自愈（带护栏）+ 最小验证

## 风险与开放问题

- bounded retry 会增加失败路径耗时，需要严格上限（attempt 次数 + backoff 上限）。
- 忽略 `NotFound` 可能掩盖“被外部程序持续删除”的路径异常；需 debug 日志以便排查。
- vault key 相关逻辑牵涉 Keychain 权限弹窗，需避免频繁触发与避免无上限重试。

## 变更记录 / Change log

- 2026-02-19: 冻结范围与验收标准，准备进入实现。
- 2026-02-19: 实装稳定性修复（index 错误分类、scan NotFound 容错、chunk_objects 批量落库、helper 下载/send_message 重试与节流、vault key cache 可失效），PR #44。
