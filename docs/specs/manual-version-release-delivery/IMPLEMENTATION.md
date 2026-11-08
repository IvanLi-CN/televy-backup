# TelevyBackup Manual Version Release Delivery 实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖、验证命令和 rollout 事实。

## Current Status

- Implementation: implemented on the manual target route; merge and publication remain owner-controlled
- Lifecycle: active
- Catalog note: target-only manual release replaces PR-label version selection

## Implementation Coverage

- Requirement coverage: Version Policy, manual preparation, target gate, completion gate, release contract, reservation provenance, claimed orphan-unbound recovery, and same-identity recovery with immutable release-asset reuse are implemented.
- Verification commands: `test-release-chain.sh`, `test-release-preparation.sh`, `test-release-completion.sh`, `test-release-reservation.sh`, `test-release-github-api.sh`, `test-release-failure-context.py`, `test-release-workflows.sh`, `test-homebrew-cask.sh`, `test-release-scripts.sh`, and `scripts/test-product-version.py` pass locally.
- Rollout facts: existing `v0.9.9` reservation, decision, and released refs were proven to have no product Tag, GitHub Release, stable consumed receipt, or release artifact receipt; reconciliation appends `release-retired/v0.9.9/<legacy-reservation-id>` and preserves every legacy ref SHA.
- Reclaim facts: a future `0.9.9` claim uses the independent `release-reservation-reclaimed/v0.9.9/<reservation-id>` and `release-decision-reclaimed/v0.9.9/<reservation-id>` namespaces, so Git's ref-file/directory constraint cannot force an overwrite.
- Orphan recovery facts: a claimed reservation with no downstream identity can append `release-orphaned/v<version>/<reservation-id>` only after trusted remote absence checks and bounded successful Release Product runs with no artifacts; the original reservation remains unchanged and a later claim uses a new reclaimed identity.

## Coverage / rollout summary

The active route will accept one owner-provided `release_target` through trusted `workflow_dispatch`, reserve before VERSION preparation, and publish only after a real merge. The former PR-label route remains historical context only.

## Remaining Gaps

- Run the active route against a new direct PR and collect CI evidence; PR #203 is historical evidence only.
- Dispatch the controlled orphan recovery for the verified `0.9.10` claim before creating the new VERSION-only release PR; the legacy `0.9.9` reconciliation route remains a separate compatibility path.
- Apply the checked-in quality-gate declaration to the repository's remote branch protection when the owner authorizes that external change.

## Related Changes

- Added the manual target Version Policy and target gate.
- Retired the PR-label release workflow and label-derived target authority.
- Bound reservation, preparation, completion, Release snapshots, and failure recovery to `release_target` and policy identity; same-identity recovery now uses append-only pending/attempt/finalized artifact receipts, verifies the exact trusted-main Release Product run and receipt-named artifact archive digest, preserves the original packaging policy SHA, and fails closed after an immutable artifact was recorded. A bound repair with a verified pending receipt and no prior source artifact can re-enter the pre-artifact build stage only after recording the new run/attempt.
- Added the claimed orphan-unbound recovery receipt, its dispatch-only workflow, reclaimed reservation selection, contract entry, and fixture coverage.

## References

- `./SPEC.md`
- `./HISTORY.md`
- `../../adr/0017-manual-version-release-target.md`
- `../../adr/0019-claimed-orphan-reservation-recovery.md`
