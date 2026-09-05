# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## Change log

- 2026-03-04: core 扫描改为 `.televyignore` 级联匹配；quick stats 对齐同一规则；新增 ignore 相关回归测试与 README 文档说明。
- 2026-03-05: backup `run.finish` 新增 ignore 汇总字段（`ignore_rule_files` / `ignore_invalid_rules`）；macOS 主界面补充 ignore 状态与历史摘要显示。
- 2026-03-05: review-loop 加固扫描容错语义：仅 root 缺失继续 fail-fast，非 root path-less `NotFound` 在 source 仍存在时按 best-effort 跳过；`source.ignore.summary` 无非法规则时降级为 `info`；补充非法规则计数与 path-less `NotFound` 回归测试。
- 2026-03-19: macOS app 启动 bundled daemon 时显式前置 `Contents/MacOS` 到 `PATH` 并固定 daemon 工作目录，修复发行版中 MTProto helper 无法定位导致的启动异常。
