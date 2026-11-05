# TelevyBackup Manual Version Release Delivery 实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖、验证命令和 rollout 事实。

## Current Status

- Implementation: contract approved; implementation pending
- Lifecycle: active
- Catalog note: target-only manual release replaces PR-label version selection

## Implementation Coverage

- Requirement coverage: pending implementation of Version Policy, manual preparation, target gate, completion gate, release contract, and recovery validation.
- Verification commands: pending implementation; the final record will include script fixtures, workflow contract tests, quality-gate validation, and Spec contract validation.
- Rollout facts: existing `v0.9.9` reservation and decision refs are immutable historical occupancy and are not rewritten or migrated by this topic.

## Coverage / rollout summary

The active route will accept one owner-provided `release_target` through trusted `workflow_dispatch`, reserve before VERSION preparation, and publish only after a real merge. The former PR-label route remains historical context only.

## Remaining Gaps

- Replace label-derived target resolution with the checked-in Version Policy.
- Remove the label workflow from active required checks and add manual target validation.
- Update release scripts, tests, workflows, quality-gate declaration, README, and domain terminology.
- Re-run live candidate and CI evidence after implementation on a new direct PR; PR #203 is historical evidence only.

## Related Changes

- None.

## References

- `./SPEC.md`
- `./HISTORY.md`
- `../../adr/0017-manual-version-release-target.md`
