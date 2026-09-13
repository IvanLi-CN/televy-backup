# PR-local VERSION Preparation

- Status: accepted
- Date: 2026-09-05

## Context

Product release identity must be reviewable before merge and must not depend on Cargo metadata,
generated snapshots, or credentials that are not part of the repository contract. Numeric allocation
and immutable ownership are defined by [ADR 0010](0010-release-identity-reservation.md); this ADR
only defines the PR-local VERSION write seam and its race guard.

## Decision

Use the root `VERSION` file as the trusted preparation record for the already allocated product
identity. After source checks and reservation pass, a trusted GitHub Actions workflow may call
GraphQL `createCommitOnBranch` with `expectedHeadOid` and the job's `GITHUB_TOKEN` to add only
VERSION to the PR branch. The commit carries source/version/intent/reservation trailers and is
accepted only when GitHub reports `commit.verification.verified`.

The normal merge commit is the only release input. Same-SHA recovery reuses the complete immutable
identity; a merge without identity requires a separate version-only release PR. GitHub ruleset and
branch-protection configuration is reconciled separately at PR-ready Step 5C.

## Consequences

- Reviewers can inspect the exact version commit in the PR before merge.
- A head race fails the native commit mutation instead of silently releasing a different source.
- Release workflows need only the scoped `GITHUB_TOKEN`; no GPG secret, dedicated account, or bypass is introduced.
- Preparation heads can run structural checks while source heads retain the complete validation matrix.
