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

### 2026-03-07: bootstrap waiver for PR #53

- Scope: PR `#53` only.
- Waived check: `Release intent label gate`
- Reason: the workflow is introduced by PR `#53` itself, and `pull_request_target` uses the base branch workflow definition. Therefore this PR cannot emit the new check until the workflow exists on `main`.
- Expiry: automatically expires once PR `#53` is merged; subsequent PRs must satisfy the full required check set.

## GitHub alignment

- Repo-local declaration is the source of truth.
- GitHub branch protection / rulesets must be reconciled to the `required_checks` set above.
- Current GitHub-side required-check configuration: `未检查`.
  - Reason: the available GitHub MCP tooling in this session can manage PRs/issues/labels/workflows, but it does not expose branch-protection / ruleset reconciliation for required checks.

## Local quality workflow

- Hooks: `lefthook`
- Rust checks are expected to stay green locally before push when practical.
- Release script contract tests are part of CI because release logic is shell-driven and easy to regress silently.
