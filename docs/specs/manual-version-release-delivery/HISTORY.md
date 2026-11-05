# TelevyBackup Manual Version Release Delivery 主题历史

> 这里记录主题局部生命周期、替换、兼容性与必要背景；完整 ADR 取舍保留在 `docs/adr/`。

## Lifecycle / Compatibility

- New active topic. It supersedes the release-target and version-selection portions of `product-version-release-chain` and the complete `pr-label-release` route.
- Existing append-only release identity refs remain readable historical facts. No existing reservation, decision, bound, consumed, or product tag ref is rewritten or deleted.

## Replacements / Background

- The former route used PR labels as the version and channel source and coupled label validation to PR completion.
- The current route uses one owner-provided `release_target` and a checked-in Version Policy; labels are not parsed by release workflows.
- The occupied `v0.9.9` reservation and decision refs are retained as an explicit migration fact. A future target collision fails closed and does not allocate a successor.

## Related Changes

- None.

## References

- `./SPEC.md`
- `./IMPLEMENTATION.md`
- `../../adr/0017-manual-version-release-target.md`
