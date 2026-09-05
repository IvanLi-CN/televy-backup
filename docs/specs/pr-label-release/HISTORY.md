# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## Change log

- 2026-03-07：按 `style-playbook` 的 `pr-label-release` 参考落地 label gate、拆分 CI 与 label-driven release。
- 2026-03-07：补充 release backfill 的 `main` ancestry 校验，并让 tag 并发竞争按幂等成功处理。
- 2026-03-07：将 `Release` workflow 改为全局串行，避免不同 merge commit 并发抢占同一 stable 版号。
- 2026-03-07：在 `CI (main)` 冻结 release intent artifact，避免 rerun/backfill 被 merge 后改标签污染。
- 2026-03-07：补充 `docs/quality-gates.md`，把 PR required checks / waiver / GitHub 对齐边界显式化。
