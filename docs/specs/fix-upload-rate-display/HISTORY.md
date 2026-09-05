# History

## Provenance

- Legacy source: `docs/plan/fh5ac:fix-upload-rate-display/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 变更记录 / Change log

- 2026-02-11：修复 MTProto helper progress 语义（仅成功后累计）+ status stream 不覆盖 daemon 速率；补齐单元测试与本地验证。
- 2026-02-11：daemon 侧修复速率采样窗口推进逻辑（仅在 `bytesUploaded` 前进时更新时间基准），避免 scan/progress 造成的速率脉冲。
- 2026-02-11：daemon 侧在主循环消费手动备份触发文件（`control/backup-now`），修复点击 Start 后不启动（trigger 未被消费）。
- 2026-02-12：daemon 侧在 status writer loop 中周期性 tick 速率采样，避免在缺少 progress callback 时速率“卡死”；补齐对应单测。
- 2026-02-13：daemon 侧把速率 tick 应用到 IPC status snapshots（GUI 默认读取 IPC），修复 UI 仍可能卡在旧速率的问题。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/fh5ac:fix-upload-rate-display/PLAN.md`.
