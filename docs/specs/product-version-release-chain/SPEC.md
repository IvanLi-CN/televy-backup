# TelevyBackup Product Version and Release Chain

## Related ADRs

- [PR-local VERSION preparation](../../adr/0004-pr-local-version-preparation.md)
- [Immutable release identity reservation](../../adr/0010-release-identity-reservation.md)
- [Independent helper bootstrap state](../../adr/0012-helper-bootstrap-state.md)
- [Same-SHA recovery bound repair](../../adr/0013-release-recovery-bound-repair.md)
- [Protected release authority](../../adr/0014-protected-release-authority.md)

## Context and Scope

This topic owns TelevyBackup's product version identity from PR labels through VERSION preparation,
mainline release, receipts, recovery, and failure context. It covers release orchestration facts and
does not change macOS application behavior, package contents, or notification transport selection.
GitHub remote policy is reconciled only at the PR-ready delivery boundary.

## Requirements

### REQ-PVR-001: Final tags are the numeric baseline

The highest eligible final product tag `vX.Y.Z` is the only numeric baseline. An eligible final
product tag must be an annotated tag created by `github-actions[bot]` under the protected
`protected-release-automation` namespace and must point to a commit reachable from `main`.
Foreign, lightweight, incomplete, or unreachable final tags fail closed. A reachable lightweight
prerelease tag from the pre-provenance era may be retained only as historical occupancy for its
same base/channel ordinal; it is never a numeric baseline, existing-tag proof, recovery identity,
or publication proof. Annotated prerelease tags with invalid provenance and unreachable tags still
fail closed. If no final tag exists, the virtual baseline is `0.0.0`. `VERSION`, Cargo manifests,
environment variables and merge order are never successor inputs. `type:major`, `type:minor` and
`type:patch` advance that baseline once; prerelease tags do not advance it.

### REQ-PVR-002: Channels have one formal grammar

Product labels use exactly one of `channel:prod`, `channel:beta`, `channel:rc`, or `channel:dev`.
`prod` writes `X.Y.Z`; the other channels write `X.Y.Z-beta.N`, `X.Y.Z-rc.N`, or `X.Y.Z-dev.N`.
The ordinal starts at one and is the next existing ordinal for the same base/channel. Local
development builds retain the short-SHA identity and are separate from formal `channel:dev`.

### REQ-PVR-003: Non-product labels are channel-free

`type:docs` and `type:skip` must have no channel. They complete successfully without writing
VERSION, creating a reservation, creating a product tag, publishing a Release, or notifying a
failure. `channel:stable`, `channel:canary`, and `type:none` are migration inputs only and are
rejected by the new gate.

### REQ-PVR-004: Reservation is the pre-merge claim

Before VERSION preparation, the trusted controller creates
`refs/tags/release-reservation/v<version>`. The ref target is a commit whose only parent is the
source SHA and whose tree equals the source tree. Its trailers record reservation id, owner, claim
key, boundary token, version, channel, and `claimed` state. First creation wins; an identical claim
is idempotent; foreign ownership, stale state, provenance mismatch and tag conflict fail closed.
Before the first `bound` or explicitly confirmed `released` transition, the controller also creates
the immutable arbitration ref `refs/tags/release-decision/v<version>`. First decision creation wins;
a decision for the other state or identity fails closed. No reservation, decision, or receipt ref is
updated, deleted, or force-pushed. Receipt creation independently
re-verifies reservation parent/tree/trailers; `bound` must exist before `consumed`, while `released`
is allowed only for an unbound claim with explicit maintainer confirmation.
Protected identity refs are appended only with the configured Protected Release Authority; the
default workflow token is not a protected-ref writer. Product annotated tags remain created by
`github-actions[bot]` so their provenance contract is unchanged.

### REQ-PVR-005: Preparation and completion preserve identity

Normal preparation uses GitHub `createCommitOnBranch` with `expectedHeadOid`, changes only VERSION,
and requires a GitHub-native verified commit. The commit records source SHA, final version, type,
channel, reservation ref, owner, claim key, boundary token, release mode and provenance. Release
completion validates those fields, source checks, ancestry, and reservation provenance.
The production completion gate additionally requires the preparation commit's GitHub-native
verification state; fixture provenance cannot enter the merge gate.

`version-only-release-pr` is a separate mode. It is a non-empty PR changing only VERSION and
records one covered merge SHA that does not already have a release identity. Its new merge SHA is
the identity; the covered merge remains historical context. No workflow automatically creates this
PR.

### REQ-PVR-006: Mainline release uses bound identity only

After merge, Release Product reads the same SHA/version/channel triple, verifies the reservation,
and appends `release-bound/v<version>/<merge-sha>`. It builds and publishes from that merge SHA,
creates the product tag without overwriting an existing ref, and appends
`release-consumed/v<version>/<merge-sha>` after publication. Before helper resolution, an already
published product Release or a verified consumed receipt is terminal; it may verify or append the
consumed receipt but MUST NOT re-enter packaging. `prod` may be the stable latest surface; beta/rc/dev
are prereleases and never update stable latest.

### REQ-PVR-007: Recovery is same-SHA or an explicit new PR

Same-SHA recovery accepts only an existing merged release identity: the exact merge SHA, the
prepared VERSION/channel identity, and its matching reservation. It retries missing bound or
consumed receipts and missing publish work. A manual recovery run may append a missing bound
receipt only after the current trusted scripts verify the reservation, preparation provenance, and
merge identity; the append-only operation never allocates a version or changes an existing ref.
Once bound exists, recovery verifies it before doing publish work. It never writes VERSION,
computes a successor, changes a channel, or retags. A historical merge with no identity is not a
recovery input; it can be released only through a new, version-only-release-pr. History scanning,
queues, trains, backfill and automatic PR creation are not part of this contract. Recovery policy evaluation uses the current trusted main workflow
scripts, while packaging and publication remain bound to the recovered merge identity.

### REQ-PVR-008: Intent snapshots and failure context are non-authoritative

Each resolved run writes and uploads `release-intent.json` containing PR/source/merge SHA, mode,
covered merge, type/channel/version/tag, helper source mode/tag, all reservation fields, provenance,
artifact names, `run_id`, `run_attempt`, run URL and recovery instruction. The failure notifier accepts
the snapshot only when both run fields match the failed `Release Product` workflow attempt; otherwise
it fails closed and reconstructs only from immutable repository facts. Reused helper resolution also uploads the exact
Universal DMG, `BUILD-MANIFEST.json`, and `SHA256SUMS` as the immutable `snapshot-helper-source`
workflow artifact consumed by every macOS build/assembly job. It is an Actions artifact snapshot, not
product code and not the only fact source. Recovery reconstructs identity from immutable Git refs,
commit trailers and product tags. Helper bootstrap state is resolved independently from the product RC
ordinal: a verified published helper Release is reused when available, invalid candidates fall back to
the next candidate, while a missing helper Release requires an explicit same-SHA bootstrap recovery. Failure
notification distinguishes publish failure, no-identity and resolver error; unresolved identity
never fabricates a version, tag, or recovery command.

The release-owning agent MUST report successful publication directly to the owner, and Release Product MUST NOT create or update a result comment on the source PR.

### REQ-PVR-009: Required gates preserve queued evaluations

`Release intent label gate` and `Release completion` are required PR gates and use a per-PR
non-preemptive `queue: max` concurrency policy. They MUST NOT use `cancel-in-progress: true`.
`Release completion` MUST fetch the current pull request through the GitHub API at execution time,
verify that its open head and base still match the event-bound SHAs, and pass that current labels
snapshot to the validator. After waiting for source checks, it MUST repeat the head/base and labels
validation immediately before invoking the completion validator. Event-payload labels are trigger
metadata, not an authoritative input for a queued completion evaluation.

### REQ-PVR-010: Protected release authority is explicit

Reservation and receipt writes MUST use the dedicated protected release authority configured for
the repository. Release workflows MUST fail before packaging when its App ID or private key is
missing. The authority MAY bypass the tag ruleset only as the explicitly configured GitHub App; the
application-level writer MUST continue to reject deletion, overwrites, foreign provenance, and
successor allocation. Product annotated tags MUST continue to be created by `github-actions[bot]`.

## Verification

### VER-PVR-001

Covers: REQ-PVR-001, REQ-PVR-002, REQ-PVR-003. `scripts/test-product-version.py`, label-gate
fixtures, and final-tag-first release-chain fixtures verify formal grammar, channel allocation and
channel-free skip behavior.

### VER-PVR-002

Covers: REQ-PVR-004. `.github/scripts/test-release-reservation.sh` verifies first-create-wins,
same-claim retry, foreign-claim rejection, and immutable receipt refs.

### VER-PVR-003

Covers: REQ-PVR-005. Preparation and completion fixtures verify VERSION-only ancestry, provenance,
expected-head workflow text, GitHub-native verification, and version-only release PR boundaries.

### VER-PVR-004

Covers: REQ-PVR-006, REQ-PVR-007. Release workflow and helper resolver contract tests verify
bound/consumed receipts, same-SHA recovery inputs, product tag ownership, prerelease publication,
independent helper bootstrap state, and no automatic history backfill.

### VER-PVR-005

Covers: REQ-PVR-008, REQ-PVR-009. Failure-context and workflow contract tests verify locked identity
payloads, no-identity/resolver-error distinction, unresolved identity fail-closed behavior, intent
artifact generation, required-gate scheduling, and current PR label revalidation.

### VER-PVR-006

Covers: REQ-PVR-010. Release workflow contract tests verify explicit App configuration checks, App
token minting for protected identity refs, default Actions provenance for product tags, and no
personal PAT fallback.

## Verification Map

| Requirement | Verification |
| --- | --- |
| REQ-PVR-001, 002, 003 | VER-PVR-001 |
| REQ-PVR-004 | VER-PVR-002 |
| REQ-PVR-005 | VER-PVR-003 |
| REQ-PVR-006, 007 | VER-PVR-004 |
| REQ-PVR-008 | VER-PVR-005 |
| REQ-PVR-009 | `.github/scripts/test-release-workflows.sh` |
| REQ-PVR-010 | VER-PVR-006 |

## Acceptance evidence

- `scripts/test-product-version.py`
- `.github/scripts/test-release-scripts.sh`
- `.github/scripts/test-release-chain.sh`
- `.github/scripts/test-release-reservation.sh`
- `.github/scripts/test-release-preparation.sh`
- `.github/scripts/test-release-completion.sh`
- `.github/scripts/test-release-workflows.sh`
- `.github/scripts/test-release-workflow-execution.sh`
- `.github/scripts/test-release-helper.sh`
- `.github/scripts/test-package-scripts.sh`
- `.github/quality-gates.json`
