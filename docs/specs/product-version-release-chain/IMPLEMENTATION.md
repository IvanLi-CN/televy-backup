# Implementation

## Components

| Component | Location |
| --- | --- |
| Formal and local version grammar | `scripts/product-version.py` |
| Final-tag-first allocation and product-tag provenance | `.github/scripts/release_chain.py`, `.github/workflows/release.yml` |
| Append-only reservation, receipt and intent snapshot | `.github/scripts/release_reservation.py` |
| Independent Snapshot Access helper source resolution | `.github/scripts/release_helper.py`, `.github/workflows/release.yml` |
| Label policy | `.github/scripts/label-gate.sh`, `.github/release-contract.json` |
| Trusted preparation | `.github/workflows/release-preparation.yml` |
| Completion gate | `.github/workflows/release-completion.yml`, `.github/scripts/release_completion.py` |
| Mainline bind, build and publish | `.github/workflows/release.yml` |
| Failure context delivery | `.github/workflows/notify-release-failure.yml` |
| Release identity ref enforcement | `.github/scripts/release_reservation.py`, `.github/release-contract.json`, `.github/workflows/release-preparation.yml`, `.github/workflows/release.yml` |
| Required-check declaration | `.github/quality-gates.json`, `docs/quality-gates.md` |

The failure resolver is bound to the exact `Release Product` workflow attempt and verifies
annotated product-tag tagger provenance before exposing a recovery candidate. The publish tag,
channel, and terminal published-release paths are exercised from the checked-in workflow `run`
blocks with a local Git/API fixture. Release state is read through the GitHub REST API using
`/releases/tags/<tag>` (`draft` and `prerelease`) and, for stable releases, `/releases/latest`
(`tag_name`) instead of CLI-specific release JSON fields.

## Identity flow

1. Label Gate validates one product type and one new channel, or channel-free docs/skip.
   Label Gate and Release completion are required per-PR gates with non-preemptive `queue: max`
   scheduling; after preparation, their manual runs target the prepared PR head so the required
   check-runs stay attached to that candidate. Dispatch mode checks out trusted `main`, verifies
   the selected branch/SHA against the current in-repository PR, and completion re-reads the
   current PR labels after verifying the queued head/base.
2. Preparation enumerates fetched product tags, requires annotated GitHub Actions provenance for
   final tags, retains reachable pre-policy lightweight prerelease tags only for ordinal occupancy,
   calculates the candidate from the highest final tag, and uses the default `GITHUB_TOKEN` to
   create the reservation before writing VERSION.
3. The same PR branch receives one GitHub verified VERSION-only commit guarded by
   `expectedHeadOid`. Later source commits may follow it; `release_chain.py find-prepared` resolves
   the first-parent preparation commit within the current `base..head` range and rejects a changed
   current VERSION.
4. Release completion freezes the reservation and provenance, and production completion verifies
   the GitHub API signature state for the resolved preparation commit. A normal PR merge creates the candidate's
   merged identity; a version-only release PR creates a new identity for one covered old merge. A
   single-parent covered commit must be an authoritatively merged main PR result, and product tags
   plus append-only identity refs must not already target it. The merge-group gate waits for all
   required check-runs on the current candidate head, while the merge-group gate waits for the
   merge-group head, refreshes the current PR identity/labels, and passes that immutable check
   snapshot to the same completion validator. The PR and merge-group gates allow up to 30 minutes
   for source required checks, covering the existing native package and Swift test matrix without
   weakening the fail-closed timeout.
5. Release Product verifies an existing published Release or consumed receipt as a terminal state
   before helper resolution. For an active release, it resolves and records either a verified helper
   Release for byte-identical reuse or an explicitly requested one-time bootstrap mode. Reused helper
   assets are downloaded and verified once in `resolve`, uploaded as `snapshot-helper-source`, and
   consumed by the native build and assembly jobs. It then verifies the merged identity, appends bound,
   builds once, creates the product tag and GitHub Release, then appends consumed with the default
   `GITHUB_TOKEN`. The append-only receipt writer rejects foreign provenance, overwrites, invalid
   transitions, and successor allocation.

## Release identity ref protection

No additional CI credential is required or permitted. The remote ruleset protects product tags
matching `refs/tags/v*` and excludes `release-reservation/*`, `release-decision/*`,
`release-bound/*`, `release-consumed/*`, and `release-released/*`. This lets both workflows use
their existing `GITHUB_TOKEN`; the application-level writer in `release_reservation.py` remains
responsible for append-only creation, idempotent same-claim retries, provenance validation, and
state ordering. Product tag creation continues to use the default Actions identity so the required
`github-actions[bot]` annotated-tag provenance is preserved.

## Recovery boundaries

The `release-intent.json` artifact is convenient run context only. A missing or expired artifact is
reconstructed from immutable Git refs, commit trailers and product tags. Same-SHA recovery requires
the existing merged identity and matching reservation; when the bound receipt is missing, the
trusted recovery path can append it atomically after re-verifying the same-SHA provenance. It
never calculates a new version or changes an existing ref. Helper bootstrap is permitted only when
no approved helper Release is available and the recovery input explicitly requests it. No identity
is reported as an unresolved state and cannot produce a fabricated tag or recovery command.
Recovery and publication keep the trusted main checkout for policy and helper scripts; the historical
recovery input remains the product identity passed through resolved outputs for packaging and
publication. The write-capable publish job does not execute policy scripts from the product
checkout. The assembly job likewise evaluates release-asset validation from the exact trusted policy
commit resolved by the workflow while keeping the recovered merge checkout as the product input.
