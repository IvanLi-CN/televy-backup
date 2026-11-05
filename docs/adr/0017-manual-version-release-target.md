# ADR 0017: Manual Release Target Authority

- Status: accepted
- Date: 2026-11-05

## Context

TelevyBackup's previous release route used PR labels to select a type and channel, then allocated a version before creating a VERSION preparation commit. That route made release authority depend on mutable PR metadata and left the repository with two concepts that were easy to confuse: the owner's intended release decision and the execution-time canonical version. It also made a collision look like an invitation to calculate a successor, which is unsafe for an immutable release identity.

The repository already has append-only reservation, decision, bound, consumed, and released refs, verified VERSION preparation, exact merge identity, and same-SHA recovery. The required change is to replace only the target authority and its PR gate while preserving those identity mechanics. The Style Playbook `manual-version-release-delivery` Topic requires a single manual target, a pluggable Version Policy, reservation before irreversible work, explicit post-merge Release, and same-identity recovery.

## Decision

Use one owner-provided `release_target` field on the trusted `Prepare product version` `workflow_dispatch` entry point. The field accepts `patch`, `minor`, `major`, or the repository's explicit final/prerelease SemVer grammar. The input remains the recorded owner intent; Version Policy resolves it to a canonical version using the highest eligible final product tag reachable from `main`, with the declared `0.0.0` virtual baseline only when no eligible final tag exists.

Version Policy is the only component allowed to resolve a new canonical version. It records a policy id, parses the single target, validates baseline ordering, rejects occupied product/identity mappings, and emits the channel and publication mapping. It never reads PR labels, changed-file names, PR count, merge order, current VERSION, compatibility classification, or a workflow artifact to choose a target. An occupied target is a conflict; the policy never allocates a successor as a recovery or retry behavior.

The reservation claim key includes the candidate PR identity and the raw owner target, while the reservation records the original source SHA and policy identity. A retry with the same PR, raw target, policy, and descendant source MAY reuse the claim. A changed target, foreign PR, unrelated source, or policy mismatch fails closed. The immutable reservation is created before VERSION preparation and before all irreversible Release work.

The release boundary remains staged: target validation and reservation, VERSION-only preparation in the PR, required checks/review, exact-head merge, bound identity, explicit Release publication, and consumed receipt. The release workflow never merges `main`, and a green PR or preparation run is not a publication result.

Same-Identity Recovery accepts only an existing reservation and merged identity with matching version, channel, policy, source/merge SHA, and provenance. It retries missing receipts or publication using the same artifact identity. A historical merge without a verified identity requires a new explicit release target and release boundary; it cannot be relabeled as recovery.

Release asset recovery uses three additional append-only refs: `release-artifact-pending` is written immediately before binding, `release-artifact-attempts` records every run/attempt authorized to produce assets, and `release-artifacts` is written after the immutable run/attempt-scoped source artifact upload and records the exact artifact name, id, digest, source run, attempt, and packaging policy SHA. Recovery must use those durable receipts and the exact artifact archive; an expired or otherwise unavailable recorded artifact fails closed, and a missing pending receipt cannot authorize a rebuild.

The PR-label workflow and label policy are retired from active release authority. Historical labels and the old policy files may remain readable for audit until separately removed, but active workflows, required checks, contracts, scripts, and notifications do not parse them.

## Alternatives Considered

### Keep PR labels as the target source

Rejected because mutable PR metadata is not the explicit owner decision required by the target-only manual contract, and it allows a label edit to change release meaning without a new target confirmation.

### Require the owner to provide only an exact `X.Y.Z`

Rejected because it moves baseline arithmetic and prerelease mapping into the human interface. Symbolic `patch`, `minor`, and `major` remain available while the policy retains responsibility for canonical resolution.

### Let a collision allocate the next available successor

Rejected because it changes the requested identity and makes retries non-auditable. A collision is an explicit conflict requiring a new owner target and, when necessary, a new release boundary.

### Publish directly from the preparation workflow

Rejected because it would collapse PR review, merge, Release, and recovery boundaries. The preparation workflow only reserves and prepares; publication remains a separate post-merge operation.

## Consequences

- Release owners must explicitly provide and confirm a target before a VERSION preparation identity exists.
- Symbolic target resolution is deterministic and testable without making the owner calculate an exact version.
- Existing immutable identity refs remain valid historical facts, and `v0.9.9` cannot be silently replaced by a successor.
- The active required-check set no longer includes a label gate; manual target validation and release completion validate commit-bound target evidence instead.
- Compatibility classification remains an optional, separate concern and does not become a release input.

## Supersession / Compatibility

This ADR supersedes the PR-label target-selection and label-gate portions of the previous release-chain contract. ADR 0010, ADR 0013, and ADR 0014 remain applicable for append-only identity refs, same-SHA bound repair, and protected product-tag authority unless this ADR explicitly changes their target input boundary.

## Evidence

- [Manual version release delivery Spec](../specs/manual-version-release-delivery/SPEC.md)
- [Product version release chain history](../specs/product-version-release-chain/HISTORY.md)
- [PR + Label release Spec](../specs/pr-label-release/SPEC.md)
- [Manual version release delivery Topic](https://github.com/IvanLi-CN/style-playbook-skills/blob/main/skills/style-playbook/references/topics/manual-version-release-delivery/TOPIC.md)
