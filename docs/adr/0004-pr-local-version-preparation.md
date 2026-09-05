# PR-local VERSION Preparation

- Status: accepted
- Date: 2026-09-05

## Context

Product release identity must be reviewable before merge and must not depend on mutable tags, Cargo metadata, generated snapshots, or credentials that are not part of the repository contract. The preparation commit also needs a race guard so a changed PR head cannot receive a version selected for another SHA.

## Decision

Use the root `VERSION` file as the product source of truth. After source checks pass, a trusted GitHub Actions workflow may call GraphQL `createCommitOnBranch` with `expectedHeadOid` and the job's `GITHUB_TOKEN` to add only VERSION to the PR branch. The commit carries source/version/intent trailers and is accepted only when GitHub reports `commit.verification.verified`.

The normal merge commit is the only release input. Recovery is limited to the same merge SHA and VERSION. GitHub ruleset and branch-protection configuration is reconciled separately and is not mutated by this repository change.

## Consequences

- Reviewers can inspect the exact version commit in the PR before merge.
- A head race fails the native commit mutation instead of silently releasing a different source.
- Release workflows need only the scoped `GITHUB_TOKEN`; no GPG secret, dedicated account, or bypass is introduced.
- Preparation heads can run structural checks while source heads retain the complete validation matrix.
