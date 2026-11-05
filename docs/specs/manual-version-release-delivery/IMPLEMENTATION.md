# TelevyBackup Manual Version Release Delivery 实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖、验证命令和 rollout 事实。

## Current Status

- Implementation: implemented on the manual target route; merge and publication remain owner-controlled
- Lifecycle: active
- Catalog note: target-only manual release replaces PR-label version selection

## Implementation Coverage

- Requirement coverage: Version Policy, manual preparation, target gate, completion gate, release contract, reservation provenance, and same-identity recovery with immutable release-asset reuse are implemented.
- Verification commands: `test-release-chain.sh`, `test-release-preparation.sh`, `test-release-completion.sh`, `test-release-reservation.sh`, `test-release-github-api.sh`, `test-release-failure-context.py`, `test-release-workflows.sh`, `test-homebrew-cask.sh`, `test-release-scripts.sh`, and `scripts/test-product-version.py` pass locally.
- Rollout facts: existing `v0.9.9` reservation and decision refs are immutable historical occupancy and are not rewritten or migrated by this topic.

## Coverage / rollout summary

The active route will accept one owner-provided `release_target` through trusted `workflow_dispatch`, reserve before VERSION preparation, and publish only after a real merge. The former PR-label route remains historical context only.

## Remaining Gaps

- Run the active route against a new direct PR and collect CI evidence; PR #203 is historical evidence only.
- Apply the checked-in quality-gate declaration to the repository's remote branch protection when the owner authorizes that external change.

## Related Changes

- Added the manual target Version Policy and target gate.
- Retired the PR-label release workflow and label-derived target authority.
- Bound reservation, preparation, completion, Release snapshots, and failure recovery to `release_target` and policy identity; same-identity recovery now reuses the original `release-assets` artifact and fails closed when it is unavailable.

## References

- `./SPEC.md`
- `./HISTORY.md`
- `../../adr/0017-manual-version-release-target.md`
