# GitHub Actions build reuse

## Observed bottlenecks

Full runs observed on November 3–4, 2026 (UTC) showed PR CI taking
12m27s–15m47s (three samples), main CI 11m28s–18m05s (four samples), and
actual automatic product releases 18m39s–25m13s (three samples).
These cross-commit observations are not controlled benchmarks.

The Swift script repeatedly compiled every application source for nine separate
test executables. The application compilation can be shared while preserving
each executable's independent entry point and process state.

Intel warm jobs restored an exact cache key created on October 14, compiled
for 3m16s–7m24s including job overhead, then skipped saving because the primary
key already existed. The consuming package job restored the same old entry.
The new compilation output never crossed the runner boundary.

Evidence:
- [PR CI](https://github.com/IvanLi-CN/televy-backup/actions/runs/41779078433)
- [Main CI](https://github.com/IvanLi-CN/televy-backup/actions/runs/41785326785)
- [Release](https://github.com/IvanLi-CN/televy-backup/actions/runs/41786818130)
- [Second release cache observation](https://github.com/IvanLi-CN/televy-backup/actions/runs/41648338173)

## Implementation boundaries

- `swift-unit-tests.sh` builds an optimized, testing-enabled application module
  once, then links each whole-application test executable against it using
  `@testable import`. Smaller source-specific tests and the production typecheck
  remain independent. All tests still execute on every invocation.
- `TELEVYBACKUP_SWIFT_TEST_BUILD_MODE=isolated` retains the original repeated
  compilation path for comparison. Generated imports and library files live
  only in the configured output directory.
- Workspace and MTProto Cargo builds run concurrently inside `build-app.sh`,
  after product build identity is exported. Both must finish successfully before
  packaging continues. No separate Intel warm runner is needed.
- macOS Cargo caches include compiler/SDK fingerprints, lockfiles and source
  identity, with compatible lockfile-scoped restore prefixes. New source builds
  can save refreshed output instead of repeatedly restoring a frozen source
  baseline. PR caches remain subject to GitHub's branch cache visibility; no
  cross-PR artifact is used as authoritative release input.
- Package cancellation applies only to superseded candidates of the same PR.
  Merge-group and manual runs do not use that cancellation rule. Long native
  and assembly jobs use `!cancelled()` rather than job-level `always()`, so
  GitHub cancellation can stop an obsolete build instead of letting it continue.
- Existing native and Universal DMG verification, Snapshot Access identity,
  signing, acceptance and release gates remain intact. Repeated DMG validation
  is retained until its persisted evidence can be safely shared.

## Measurement and acceptance

The agreed target is a 30% reduction in median complete PR/main/release waiting
with no increase in total runner time for equivalent workloads. This is an
acceptance target, not an achieved result. Use at least five comparable samples
per run class and record source SHA, platform, workload, queue/pending time,
step durations, runner total and cold/warm/partial cache state separately.
Do not combine prepared identity-only runs or skipped releases with full builds.

`CI performance probes` is a separate diagnostic workflow. Editing that workflow
opts a PR into the experiment; it also accepts manual dispatch. It runs five
paired Swift comparisons, at most two simultaneously, alternating mode order.
Each mode has a fresh output directory and must produce the same success
markers. Artifacts contain full logs and timing JSON. SDK module caches are
ambient runner state and are explicitly recorded as such; these probes alone
cannot establish a cold/warm Actions-cache or complete-workflow speed claim.
Probe jobs are diagnostic, not substitutes for automatic required PR checks.

For full-workflow comparisons include all required PR workflows, not only
`CI (PR)`. Compare event-to-gate time and execution-to-gate time independently.
Observe main and product release performance only on their actual identities;
pre-merge probes never authorize publishing a product release. Any missing
phase evidence remains pending, rather than being recorded as a performance
pass. Do not weaken required gates to reach the timing target.

Rollback is a revert of the build scripts/workflow changes. There is no product
API or persistent-state migration.
