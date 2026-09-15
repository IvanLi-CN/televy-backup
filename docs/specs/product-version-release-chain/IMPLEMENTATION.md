# Implementation

## Components

| Component | Location |
| --- | --- |
| Formal and local version grammar | `scripts/product-version.py` |
| Final-tag-first allocation and provenance | `.github/scripts/release_chain.py` |
| Append-only reservation, receipt and intent snapshot | `.github/scripts/release_reservation.py` |
| Independent Snapshot Access helper source resolution | `.github/scripts/release_helper.py`, `.github/workflows/release.yml` |
| Label policy | `.github/scripts/label-gate.sh`, `.github/release-contract.json` |
| Trusted preparation | `.github/workflows/release-preparation.yml` |
| Completion gate | `.github/workflows/release-completion.yml`, `.github/scripts/release_completion.py` |
| Mainline bind, build and publish | `.github/workflows/release.yml` |
| Failure context delivery | `.github/workflows/notify-release-failure.yml` |
| Required-check declaration | `.github/quality-gates.json`, `docs/quality-gates.md` |

## Identity flow

1. Label Gate validates one product type and one new channel, or channel-free docs/skip.
2. Preparation enumerates fetched product tags and reservation refs, calculates the candidate from
   the highest final tag, and creates the reservation before writing VERSION.
3. The same PR branch receives one GitHub verified VERSION-only commit guarded by
   `expectedHeadOid`.
4. Release completion freezes the reservation and provenance, and production completion verifies
   the GitHub API signature state for the prepared commit. A normal PR merge creates the candidate's
   merged identity; a version-only release PR creates a new identity for one covered old merge.
5. Release Product verifies an existing published Release or consumed receipt as a terminal state
   before helper resolution. For an active release, it resolves and records either a verified helper
   Release for byte-identical reuse or an explicitly requested one-time bootstrap mode. Reused helper
   assets are downloaded and verified once in `resolve`, uploaded as `snapshot-helper-source`, and
   consumed by the native build and assembly jobs. It then verifies the merged identity, appends bound,
   builds once, creates the product tag and GitHub Release, then appends consumed.

## Recovery boundaries

The `release-intent.json` artifact is convenient run context only. A missing or expired artifact is
reconstructed from immutable Git refs, commit trailers and product tags. Same-SHA recovery requires
the existing bound identity and never calculates a new version. Helper bootstrap is permitted only
when no approved helper Release is available and the recovery input explicitly requests it. No
identity is reported as an unresolved state and cannot produce a fabricated tag or recovery command.
Recovery keeps the trusted main checkout for policy and helper scripts; the historical recovery
input remains the product identity passed through resolved outputs for packaging and publication.
The assembly job likewise evaluates release-asset validation from the exact trusted policy commit
resolved by the workflow while keeping the recovered merge checkout as the product input.
