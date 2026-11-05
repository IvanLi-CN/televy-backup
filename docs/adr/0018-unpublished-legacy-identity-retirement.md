# ADR 0018: Unpublished Legacy Identity Retirement

- Status: accepted
- Date: 2026-11-05

## Context

The manual release route found an immutable `0.9.9` reservation, decision, and released receipt from an earlier preparation attempt. The remote repository has no stable `v0.9.9` product tag, no GitHub Release for that tag, no stable `release-consumed` receipt, and no release artifact receipt or source artifact from the checked Release Product runs. The old refs are still valuable audit history, but their exact reservation ref is already occupied and cannot be used as a directory parent for a new child ref in Git.

## Decision

Provide a trusted, dispatch-only reconciliation workflow that accepts an explicit version, legacy reservation id, and bounded list of Release Product run ids. It must verify the product Tag and GitHub Release are absent, stable consumed and artifact refs are absent, and every checked Release Product run has no artifacts before writing anything.

The reconciliation writer appends `refs/tags/release-retired/v<version>/<legacy-reservation-id>` as a commit whose parent is the old released receipt. Its trailers record the exact source refs and SHAs, evidence digest, evidence main SHA, checked run ids, absence facts, owner, reason, and immutable retirement provenance. The old reservation, decision, and released refs are never deleted, updated, force-pushed, or replaced.

After a verified retirement, a new claim for the same version uses `refs/tags/release-reservation-reclaimed/v<version>/<reservation-id>` and the matching `refs/tags/release-decision-reclaimed/v<version>/<reservation-id>` path. Bound, consumed, released, and artifact refs retain their normal merge-identity paths. The Version Policy treats the version as available only when the retirement receipt itself proves the legacy source refs and absence facts; a product Tag or any new active identity ref still occupies the version.

The retirement and reclaimed writers remain append-only and idempotent. A missing, malformed, conflicting, or incomplete retirement receipt leaves the version occupied and fails closed. There is no rollback operation that removes the retirement receipt or restores the old claim as active.

## Consequences

- The old `0.9.9` identity remains inspectable and its original SHAs remain unchanged.
- The new release claim has a distinct Git ref namespace that is compatible with Git's ref storage rules.
- Reconciliation is auditable from one receipt and a canonical evidence digest, while publication still requires the normal merged SHA, GitHub Release, product Tag, and consumed receipt chain.
- A mistaken or incomplete remote fact check stops before any ref is written; a successful retirement is an irreversible append-only audit decision.

## Evidence

- [Manual version release delivery Spec](../specs/manual-version-release-delivery/SPEC.md)
- [Immutable Release Identity Reservation](0010-release-identity-reservation.md)
- [Protected Release Authority](0014-protected-release-authority.md)
