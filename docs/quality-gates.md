# Quality gates

This repo treats PR checks as an explicit merge contract, not just “whatever happens to run on GitHub”.

## Baseline policy

- `baseline_policy`: `explicit-waiver-required`
- Rule: every declared required check must be green before merge unless there is a documented waiver.

## Required checks

Use the exact GitHub check names below as the merge gate contract for PRs targeting `main`:

- `quality`
- `macOS Swift tests`
- `Release intent label gate`

## Informational checks

- None declared.

## Expected PR workflows

- `CI (PR)`
  - `quality`
  - `macOS Swift tests`
- `PR Label Gate`
  - `Release intent label gate`

## Waivers

- None.

## Bootstrap note

- PR `#53` introduces the `pull_request_target` workflow that backs `Release intent label gate`.
- Until that workflow exists on `main`, bootstrap validation for PR `#53` is satisfied by manually dispatching `PR Label Gate` from the PR branch with `pr_number=53`.
- After PR `#53` merges, subsequent PRs are validated automatically through `pull_request_target`.

## GitHub alignment

- Repo-local declaration is the source of truth.
- GitHub branch protection / rulesets must be reconciled to the `required_checks` set above.
- Current GitHub-side required-check configuration: `存在`.
  - Verified on GitHub Settings > Branches on 2026-03-08.
  - Enforced required checks: `quality`, `macOS Swift tests`, `Release intent label gate`.
  - `Require a pull request before merging` is also enabled for `main`.

## Local quality workflow

- Hooks: `lefthook`
- Rust checks are expected to stay green locally before push when practical.
- Release script contract tests are part of CI because release logic is shell-driven and easy to regress silently.
