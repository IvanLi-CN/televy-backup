# Immutable Release Identity Reservation

- Status: accepted
- Date: 2026-09-14

## Context

The previous release flow treated the committed `VERSION` as a version counter. That allows two
parallel PRs to derive the same successor, makes prerelease channels ambiguous, and leaves a failed
publish without a durable proof of which claim owns the version. Git notes, a control branch, and an
external store would add a second state system and are not available to every clone or reviewer.

## Decision

Use the highest eligible final product tag as the numeric baseline. Allocate `prod`, `beta`, `rc`,
and formal `dev` identities from that baseline and the existing product tag namespace. Before the
VERSION-only preparation commit, create one immutable repository tag under
`refs/tags/release-reservation/v<version>`. The reservation is a commit with the source commit as
its only parent, the source tree as its tree, and trailers containing the reservation id, owner,
claim key, boundary token, version, channel, and `claimed` state.

After merge, append immutable `release-bound/v<version>/<merge-sha>` and, after publication,
`release-consumed/v<version>/<merge-sha>` receipts. A controlled maintainer action may append
`release-released/v<version>/<reservation-id>` only for an unmerged claim. Existing refs are never
updated or deleted: an identical claim is idempotent, while a foreign owner, claim, provenance, or
state fails closed.

The first post-reservation state transition also creates one immutable
`release-decision/v<version>` ref. Its append-only creation arbitrates the mutually exclusive
`bound` and `released` paths before their state-specific receipt refs are created; an existing
decision with another state or identity fails closed. This closes the cross-ref race that separate
receipt names cannot solve by read-before-create checks alone.

Every receipt writer independently verifies the reservation's parent, tree, and trailers. The
`bound` receipt must exist before `consumed` is created; a recovery run may only verify an existing
`bound` receipt and may not create the first one. A `released` receipt is rejected once a bound or
consumed receipt exists.

The release intent JSON is an Actions artifact snapshot only. Recovery reconstructs identity from
reservation and receipt refs, preparation/merge trailers, and the product tag. The normal PR mode,
the explicit `version-only-release-pr` mode, and same-SHA recovery remain separate contracts.

## Consequences

- Version allocation is race-safe without Git notes, a control branch, a database, object storage,
  or a new service.
- Reviewers can inspect the reservation and receipts in the repository's normal ref namespace.
- A failed publish retains the same merge SHA/version/channel and can be retried without allocating
  a successor.
- Reservation tags and product tags need append-only GitHub protection; insufficient permissions
  fail closed rather than weakening the protection model.
