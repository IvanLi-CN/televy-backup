# Release Identity Ref Protection

- Status: accepted
- Date: 2026-09-16

Release CI must use the existing default `GITHUB_TOKEN`. The repository must not add a PAT,
GitHub App, deploy key, or any other credential solely to write release identity refs.

Keep server-side protection on product tag refs (`refs/tags/v*`) so a published product tag cannot
be deleted, force-updated, or created without the required annotated-tag provenance. Exclude the
release identity namespaces from that product-tag ruleset: `release-reservation/*`,
`release-decision/*`, `release-bound/*`, `release-consumed/*`, and `release-released/*`.
The existing `release_reservation.py` writer is the application-level authority for those refs:
it permits only append-only creation, identical-claim retries, and valid state transitions after
independent provenance checks.

## Considered Options

- Adding a second CI credential would preserve server-side protection for identity refs but violates
  the repository's credential boundary and creates a new rotation and bypass surface.
- Requiring a maintainer to append every identity ref avoids the ruleset change but makes the release
  chain partially manual and is outside the existing automated flow.
- Keeping the identity namespaces in the product-tag ruleset makes the default token fail with
  `403 Resource not accessible by integration`, so the workflow cannot complete automatically.

## Consequences

Product tags retain server-level protection, while identity refs rely on repository code, workflow
permissions, and the append-only writer rather than server-level immutability. The remote ruleset
must therefore cover the product tag namespace and exclude the five identity namespaces before
automated release can succeed. This code change does not mutate remote GitHub settings. Same-SHA
recovery remains the only repair path; a failed receipt never allocates a successor version.
