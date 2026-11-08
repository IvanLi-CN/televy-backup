# TelevyBackup Manual Version Release Delivery 主题历史

> 这里记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- New active topic. It supersedes the release-target and version-selection portions of `product-version-release-chain` and the complete `pr-label-release` route.
- Existing append-only release identity refs remain readable historical facts. No existing reservation, decision, bound, consumed, or product tag ref is rewritten or deleted.

## Replacements / Background

- The former route used PR labels as the version and channel source and coupled label validation to PR completion.
- The current route uses one owner-provided `release_target` and a checked-in Version Policy; labels are not parsed by release workflows.
- The occupied `v0.9.9` reservation, decision, and released refs remain immutable historical facts. Because remote evidence proves no product publication or consumed artifact, the controlled reconciliation route may append a `release-retired` receipt; a new claim then uses independent reclaimed reservation and decision namespaces without rewriting the old refs.

## Related Changes

- v0.9.10 正式版沿用已验证的 v0.9.10-rc.1 修复内容，通过独立 VERSION preparation PR 发布；该条目仅记录发布链路，不改变产品行为。

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
- `../../adr/0017-manual-version-release-target.md`
