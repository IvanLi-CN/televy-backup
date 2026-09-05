# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 实现前置条件（Definition of Ready / Preconditions）

- 已确认 CLI flag：`--no-remote-index-sync`（默认不传则启用远端对齐）。
- 已确认“远端 latest 检查”策略：每次备份启动都读取 pinned catalog；仅当本地缺失/判定 stale 时才下载远端索引并替换本地。
- 已确认 pinned catalog 异常时的策略：
  - 若 pinned 不是 TelevyBackup catalog（framing invalid）：忽略并允许覆盖（输出告警）。
  - 若 pinned 看似是 catalog 但解密/解析失败：阻断并返回 `bootstrap.decrypt_failed`（提示导入正确 master key：TBK1）。


## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit: 远端 latest 与本地 index 状态判定（missing/stale/match）。
- Integration: 使用 mock storage/pinned storage 验证：
  - 本地缺失 → 自动下载并替换；
  - pinned 非 catalog → 忽略并覆盖（输出告警）；
  - pinned 解密/解析失败（疑似 catalog，但 key 不匹配/损坏）→ 阻断并返回 `bootstrap.decrypt_failed`；
  - `--no-remote-index-sync` → 不触发下载。

### Reliability

- 本地索引写入必须原子替换，避免写一半导致 DB 损坏。
- 失败路径必须可重试（不进入 scan；不破坏既有本地索引）。


## 实现里程碑（Milestones）

- [x] M1: Core 抽取并复用“download remote index db（manifest → sqlite）”能力（支持原子落盘）
- [x] M2: CLI `backup run` 接入 preflight index sync（默认启用 + 开关）
- [x] M3: pinned catalog 覆盖策略收敛（非 catalog 可覆盖；decrypt/parse 失败默认拒绝覆盖 + 明确错误指引）
- [x] M4: 测试覆盖（unit + integration）
- [x] M5: 文档与 release notes 更新

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0012:remote-first-index-sync/PLAN.md`.
