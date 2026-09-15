# ADR 0013: Same-SHA Recovery Bound Repair

## Status

Accepted. This decision supersedes the recovery detail in [ADR 0010](0010-release-identity-reservation.md).

## Context

Release Product can fail after the merged preparation identity and reservation have been verified
but before it appends the `release-bound` receipt. A strict recovery precondition that requires the
receipt to already exist strands that otherwise authoritative release identity and forces an
unnecessary new release path.

## Decision

An explicit `workflow_dispatch` recovery for the exact merged SHA may append a missing
`release-bound/v<version>/<merge-sha>` receipt through `release_reservation.py receipt --state
bound`. Before the append, the trusted main workflow verifies the preparation and merge identity,
the reservation's parent, tree, and trailers, and the requested version/channel fields. The
receipt writer creates the decision and bound refs append-only and is idempotent under a race;
existing mismatched refs fail closed.

The recovery never allocates a version, changes `VERSION`, changes the reservation, computes a
successor, retags a merge, or scans history. After the bound receipt exists, publication and the
`consumed` transition retain their existing same-SHA gates. Normal `workflow_run` publication uses
the same receipt writer and remains append-only.

## Consequences

A transient failure before receipt creation can be recovered without changing the release identity.
The recovery surface remains limited to an owner-dispatched exact SHA and is protected by the same
provenance checks as the normal bound transition. A merge with no verified preparation identity is
still not recoverable and must use a new version-only release PR.
