# TelevyBackup Manual Version Release Delivery

## Related ADRs

- [PR-local VERSION preparation](../../adr/0004-pr-local-version-preparation.md)
- [Immutable release identity reservation](../../adr/0010-release-identity-reservation.md)
- [Same-SHA recovery bound repair](../../adr/0013-release-recovery-bound-repair.md)
- [Release identity ref protection](../../adr/0014-protected-release-authority.md)
- [Manual release target authority](../../adr/0017-manual-version-release-target.md)
- [Unpublished legacy identity retirement](../../adr/0018-unpublished-legacy-identity-retirement.md)
- [Claimed orphan reservation recovery](../../adr/0019-claimed-orphan-reservation-recovery.md)

## Context and Scope

This topic owns TelevyBackup's target-only manual version release boundary from owner decision through PR preparation, merge, explicit Release publication, and same-identity recovery. It preserves the repository's append-only identity refs and macOS packaging contract while replacing PR-label version selection with one explicit manual target.

This topic does not classify API compatibility, infer release impact from changed files, grant merge authority, grant production-release authority, or select notification transport. PR labels may exist as unrelated metadata, but they are never release target input, version selection evidence, or Release eligibility evidence.

## Domain Terms

**Release Target** is the single owner-provided `release_target` value that expresses either a symbolic SemVer operation or an explicit final/prerelease version. It is intent, not the canonical version.

**Version Policy** is the checked-in policy that parses the Release Target, verifies the trusted baseline, resolves the canonical version, rejects collisions, and maps the version to identity and publication surfaces.

**Canonical Version** is the exact version returned by Version Policy for one validated Release Target. It is the value written to `VERSION`, the suffix of the product tag, and the version component of the release identity.

**Reservation** is the append-only repository claim created for the canonical version before VERSION preparation or any irreversible publication action. It is not a queue entry, mutable lock, or successor allocator.

**Merged Release Identity** is the immutable tuple `(merge SHA, canonical version, release channel, policy id, reservation id)` that names one mainline release boundary.

**Release** is the explicit post-merge publication operation that consumes the Merged Release Identity and publishes the bound artifacts. A green PR check is not a Release.

**Same-Identity Recovery** is a retry for the exact existing Merged Release Identity, including its canonical version, merge SHA, reservation, and artifact digests. It never computes or claims a successor.

## Requirements

### REQ-MVR-001: A single manual target is the only version input

The release preparation entry point MUST require one non-empty `release_target` input supplied by the release owner through `workflow_dispatch`. The supported symbolic values are `patch`, `minor`, and `major`; the supported explicit values are the repository's canonical final or prerelease SemVer grammar. The workflow MUST reject missing, duplicated, conflicting, or malformed target inputs.

Version selection MUST NOT read PR labels, PR count, changed-file names, commit messages, PR history, merge order, current `VERSION`, or compatibility classification. `pr_number`, `release_mode`, and `covered_merge_sha` identify the delivery boundary and recovery mode; none of them selects a version.

Covers: [ADR 0017](../../adr/0017-manual-version-release-target.md).

### REQ-MVR-002: Version Policy resolves and validates the canonical version

Version Policy MUST declare its policy id, target grammar, trusted baseline source, ordering rule, collision rule, reservation key, identity fields, publication mapping, and recovery rule in `.github/release-contract.json` and executable validators.

The normal numeric baseline MUST be the highest eligible final product tag reachable from `main` and carrying the repository's required protected automation provenance. When no eligible final tag exists, the policy MUST use the declared virtual baseline `0.0.0`. `VERSION`, Cargo metadata, workflow artifacts, labels, and merge order MUST NOT act as fallback baselines.

A symbolic target MUST be resolved deterministically against that baseline. An explicit target MUST be validated against the same baseline and grammar. A new target at or below the trusted final baseline, or a target whose publication identity is already owned, MUST fail closed before preparation; it MUST NOT be converted into a successor target. The only exceptions are separately authorized recovery workflows that append an immutable orphan-unbound receipt for a claimed reservation with no downstream identity, or an immutable retirement receipt after proving that an old claim has no product Tag, GitHub Release, stable consumed receipt, or release artifact receipt.

Covers: [ADR 0017](../../adr/0017-manual-version-release-target.md).

### REQ-MVR-003: Owner confirmation is bound to one candidate and one target

Preparation MUST resolve an open, same-repository PR targeting `main`, read its current head and base through the GitHub API, and bind the owner-provided `release_target` to that candidate before writing `VERSION`. The trusted preparation workflow MUST execute policy and release scripts from the trusted `main` checkout and MUST verify the live PR head before any branch mutation.

The preparation commit MUST record the raw Release Target, policy id, canonical version, release channel, source SHA, reservation id, reservation ref, claim key, boundary token, and release mode in commit trailers. A retry MUST provide the same raw target and PR identity. If the source head advances along the reserved source ancestry, the existing reservation MAY be reused without changing its source or allocating a successor; a changed target, unrelated source, or changed policy identity MUST fail closed.

Covers: [ADR 0004](../../adr/0004-pr-local-version-preparation.md), [ADR 0017](../../adr/0017-manual-version-release-target.md).

### REQ-MVR-004: Reservation precedes irreversible release work

The controller MUST create or verify `refs/tags/release-reservation/v<version>` before creating the VERSION preparation commit, product tag, GitHub Release, uploading release artifacts, or deploying them. If that exact ref is an orphan-unbound claim with a valid `release-orphaned` receipt, or an unpublished legacy claim with a valid retirement receipt, a new claim MUST use the independent `refs/tags/release-reservation-reclaimed/v<version>/<reservation-id>` namespace and its matching `release-decision-reclaimed` namespace. The reservation MUST be append-only and MUST prove its source parent, source tree, owner, claim key, raw target, canonical version, channel, policy id, reservation id, and claimed state.

An identical claim MAY be retried idempotently. A foreign claim, different source, different raw target, different policy, different owner, invalid provenance, or occupied product identity MUST stop the run. Existing reservation, decision, bound, consumed, released, orphan, retirement, and artifact refs MUST never be updated, deleted, force-pushed, or silently replaced by a successor.

Covers: [ADR 0010](../../adr/0010-release-identity-reservation.md), [ADR 0017](../../adr/0017-manual-version-release-target.md).

### REQ-MVR-005: PR, checks, merge, and Release are separate boundaries

The source candidate MUST pass the repository's declared PR checks and review policy before merge. The target gate MUST validate the preparation trailers and canonical VERSION from the trusted base; it MUST not inspect labels to decide whether a release exists. The completion gate MUST validate the current PR head/base, source checks, preparation provenance, reservation provenance, and exact target identity.

Release preparation MUST NOT merge a PR or write `main`. The post-merge Release workflow MUST require a real merge boundary and exact merge SHA, then append the bound identity before packaging and publication. Release publication MUST be an explicit stage after merge; PR green, preparation success, tag existence, and deployment success are distinct facts.

Covers: [ADR 0017](../../adr/0017-manual-version-release-target.md).

### REQ-MVR-006: Publication surfaces reuse one merged identity

The publication mapping MUST derive the product tag `v<canonical-version>`, GitHub Release prerelease/latest flags, artifact names, provenance records, and receipt refs from the same Merged Release Identity. Every artifact digest MUST remain bound to the merge SHA and policy id. Existing product tags or Releases may be treated as terminal only when their target, provenance, channel mapping, and identity match exactly; mismatches MUST fail closed.

Consumers and deployment stages MUST reuse the artifacts bound to that identity. A downstream retry MUST not rebuild a different source or select a newer tag as an implicit replacement.
Same-identity recovery MUST verify the durable finalized artifact receipt, the exact trusted `Release Product` run and receipt-named artifact metadata, the unexpired run/attempt-scoped `release-assets-<run-id>-<run-attempt>` artifact, its archive digest, manifest, checksums, merge SHA, and original packaging policy SHA before reusing those bytes. A finalized or expired source artifact MUST make recovery fail closed; recovery MUST NOT rebuild native packages or reassemble a new release asset set. A bound repair MAY rebuild the pre-artifact stage only after appending a run/attempt receipt and proving that every recorded attempt has no source artifact; it MUST preserve the same merge identity and record the current recovery execution policy separately from the packaging policy carried by the product assets.

Covers: [ADR 0017](../../adr/0017-manual-version-release-target.md), [ADR 0014](../../adr/0014-protected-release-authority.md).

### REQ-MVR-007: Recovery reuses the exact identity

Same-Identity Recovery MUST require an existing reservation plus the exact canonical version, channel, policy id, source/merge SHA, and matching identity provenance. It MAY repair a missing bound receipt, retry publication, or retry a missing consumed receipt. It MUST NOT recalculate the baseline, reinterpret the raw target, change `VERSION`, change channel, rewrite a tag, or allocate a successor.

A historical merge without a verified preparation and reservation is not recoverable under this topic. It requires a new explicit Release Target and a new release boundary, such as a non-empty VERSION-only PR when the project contract permits that mode. A claimed reservation with no downstream identity follows the separate orphan-unbound recovery route: it requires an exact reservation match, absent publication and later identity refs, bounded successful Release Product runs with no artifacts, and then a new reclaimed claim; it is not same-SHA recovery.

Covers: [ADR 0013](../../adr/0013-release-recovery-bound-repair.md), [ADR 0017](../../adr/0017-manual-version-release-target.md).

### REQ-MVR-008: PR labels are retired from release authority

The repository MUST remove the PR-label release workflow and MUST NOT declare a label gate as a required check for release preparation or completion. Existing `type:*`, `channel:*`, or legacy labels MAY remain on historical PRs, but no release script, workflow, contract, or notification path may parse them as the source of a target, version, channel, release action, or release eligibility.

The checked-in release contract and quality-gate declaration MUST identify manual target validation as the active route and MUST identify the PR-label route as retired compatibility history only.

Covers: [ADR 0017](../../adr/0017-manual-version-release-target.md).

### REQ-MVR-009: Contract, validators, and evidence stay aligned

The release contract, Version Policy validator, reservation/preparation/completion scripts, workflow contracts, release tests, quality-gate declaration, README guidance, Spec, ADR, and domain glossary MUST describe the same target-only manual flow. Tests MUST cover symbolic and explicit target parsing, baseline validation, occupied-target rejection, same-claim retry, source-descendant retry, foreign-claim rejection, no-successor behavior, post-merge Release gating, same-identity recovery, and the absence of label-based version selection.

Covers: [ADR 0017](../../adr/0017-manual-version-release-target.md).

## State Model

The normal state path is `unclaimed -> target-validated -> reserved -> prepared -> pr-ready -> merged -> bound -> released -> consumed`. An unpublished legacy claim may take the separate path `legacy-occupied -> retirement-verified -> reclaimed-reserved` only through the trusted reconciliation workflow. A claimed orphan may take the separate path `claimed-orphan -> orphan-recovery-verified -> reclaimed-reserved` only through the trusted orphan recovery workflow. `target-validated` records only the owner input and policy result; it does not publish anything. `reserved` is the first durable version claim. `prepared` is a VERSION-only verified commit in the candidate PR. `pr-ready` means required checks and review are satisfied. `merged` creates the mainline boundary. `bound` proves the merge identity before packaging. `released` proves the explicit Release publication. `consumed` proves the publication receipt and artifact binding.

Retry is allowed only within the same state identity. A reservation collision, target mismatch, provenance mismatch, missing required check, changed PR base/head, or missing merge identity is a fail-closed state, not a reason to choose a successor.

## Interfaces

### Manual preparation dispatch

- `pr_number`: required open in-repository PR targeting `main`.
- `release_target`: required single symbolic or explicit target; the only version input.
- `release_mode`: `normal` or the explicitly supported VERSION-only recovery boundary.
- `covered_merge_sha`: required only for the VERSION-only boundary and never a target selector.

The dispatch is evaluated against trusted `main`; the candidate PR is read through the GitHub API and its exact head/base are revalidated before reservation and before the VERSION commit.

### Orphan-unbound recovery dispatch

- `version`: required canonical product version carried by the original reservation.
- `reservation_id`: required original reservation identity.
- `release_target`: required exact raw target carried by the original reservation.
- `claim_key`: required original `pr:<number>:target:<release-target>` claim key.
- `boundary_token`: required original reservation boundary token.
- `release_run_ids`: required bounded list of successful `Release Product` runs checked for zero artifacts.
- `reason`: bounded operator reason recorded in the append-only receipt.

The workflow is evaluated from trusted `main`. It MUST verify absent product publication, absent later identity refs, exact original reservation provenance, and each supplied run before calling `recover-orphan`. A successful receipt permits only a new reclaimed reservation and a new VERSION-only release PR.

### Preparation trailers

The preparation commit MUST include `Release-Target`, `Release-Policy`, `Product-Version`, `Release-Channel`, `Release-Source-SHA`, `Release-Reservation-Id`, `Release-Reservation-Ref`, `Release-Reservation-Owner`, `Release-Claim-Key`, `Release-Boundary-Token`, `Release-Mode`, and `Release-Provenance`.

### Identity refs

The existing append-only namespaces remain the durable identity surface: `release-reservation`, `release-decision`, `release-reservation-reclaimed`, `release-decision-reclaimed`, `release-bound`, `release-consumed`, `release-released`, `release-orphaned`, `release-retired`, `release-artifact-pending`, `release-artifact-attempts`, and `release-artifacts`. Product tags remain `v<canonical-version>` and are never rewritten.

## Failure Semantics

- Missing `release_target`: reject before baseline resolution or reservation.
- Invalid symbolic or explicit target: reject before reservation.
- Missing or untrusted final baseline: reject unless the declared virtual-baseline bootstrap applies.
- Occupied version, tag, reservation, or publication mapping: reject without successor allocation.
- Legacy reservation reconciliation: require absent product Tag and GitHub Release, no stable consumed or artifact refs, checked Release Product runs with no artifacts, exact legacy reservation/decision/released provenance, and a new top-level reclaimed identity path; never delete or rewrite the legacy refs.
- Claimed orphan reservation recovery: require the exact claimed reservation, absent product Tag and GitHub Release, no decision/reclaimed/bound/consumed/released/retired/orphan/artifact refs, bounded successful Release Product runs with no artifacts, and a new `release-orphaned` receipt; never delete or rewrite the original reservation.
- Same claim and same target: safe idempotent retry.
- Same PR with descendant source: reuse the original reservation; never mutate it.
- Different target, source branch identity, policy id, or owner: explicit conflict.
- Missing required check or review: stop before merge.
- Merge absent: no Release operation is authorized.
- Merge present but identity absent: no recovery; require a new explicit release boundary.
- Publication or notification failure after binding: retry the same identity and artifact digests.

## Verification

### VER-MVR-001

Covers: REQ-MVR-001 and REQ-MVR-002. Version Policy unit tests and release-chain fixtures verify symbolic/explicit target parsing, trusted baseline resolution, canonical formatting, ordering, and occupied-target rejection.

### VER-MVR-002

Covers: REQ-MVR-003 and REQ-MVR-004. Preparation and reservation fixtures verify target trailers, deterministic claims, append-only refs, same-claim retries, descendant source retries, and foreign-claim conflicts.

### VER-MVR-003

Covers: REQ-MVR-005 and REQ-MVR-006. Workflow contract tests verify the manual dispatch boundary, trusted checkout, PR/check/merge separation, post-merge Release trigger, exact identity binding, publication mapping, and artifact reuse.

### VER-MVR-004

Covers: REQ-MVR-007 and REQ-MVR-008. Recovery fixtures verify same-identity-only behavior, claimed orphan recovery, reclaimed namespace isolation, and contract tests verify that label files, label workflows, and label-derived version paths are absent from the active release route.

### VER-MVR-005

Covers: REQ-MVR-009. The release contract checker, quality-gate checker, workflow tests, script tests, Spec contract checker, and documentation drift checks verify aligned current truth.

## Verification Map

| Requirement | Verification |
| --- | --- |
| REQ-MVR-001, REQ-MVR-002 | VER-MVR-001 |
| REQ-MVR-003, REQ-MVR-004 | VER-MVR-002 |
| REQ-MVR-005, REQ-MVR-006 | VER-MVR-003 |
| REQ-MVR-007, REQ-MVR-008 | VER-MVR-004 |
| REQ-MVR-009 | VER-MVR-005 |

## References

- `.github/release-contract.json`
- `.github/workflows/release-preparation.yml`
- `.github/workflows/release-completion.yml`
- `.github/workflows/release.yml`
- `.github/scripts/release_chain.py`
- `.github/scripts/release_reservation.py`
- `.github/scripts/release_preparation.py`
- `.github/scripts/release_completion.py`
- `CONTEXT.md`
- `docs/quality-gates.md`
