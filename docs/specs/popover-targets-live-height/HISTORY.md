# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/specs/README.md`：新增规格索引。
- `docs/specs/popover-targets-live-height/SPEC.md`：记录口径与验收。


## 变更记录（Change log）

- 2026-02-25: 创建规格并冻结修复口径。
- 2026-02-25: 完成实现并推送 `th/2e73n-popover-targets-live-height`，PR #48 更新到 `0d63324`。
- 2026-02-25: CI run #251 (`.github/workflows/ci.yml`) 成功；review-loop 结论为无阻塞问题。
- 2026-03-05: Popover 高度改为 `NSHostingController.sizeThatFits(in:)` 实测驱动，补齐 SwiftUI 布局尺寸测量自动化测试并接入 macOS CI，防止高度回归与多余空白复现。
