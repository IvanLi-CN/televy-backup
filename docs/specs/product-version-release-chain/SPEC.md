# TelevyBackup VERSION-only Release Chain

> Canonical topic retained as the canonical source for current product behavior.

## Related ADRs

- [PR-local VERSION preparation](../../adr/0004-pr-local-version-preparation.md)

## Requirements

### REQ-PVR-001: VERSION is the product version authority

The root `VERSION` file is the only numeric product-version source. It contains exactly one LF-terminated stable `X.Y.Z` or release-candidate `X.Y.Z-rc.N` value. Cargo package metadata remains non-authoritative package metadata and must not be used as a fallback.

Covers: G1, G2.

### REQ-PVR-002: Development and release identities are deterministic

`scripts/product-version.py` MUST resolve development identity as the next patch of the committed VERSION plus `-dev.<short-sha>`. Release identity MUST equal the committed VERSION. Rust binaries, plist values, DMG names, tools archives, Universal bundles, and manifests MUST consume the same resolver result.

Covers: G1, G2, A1, A2, A4.

### REQ-PVR-003: Labels have an exact release action

`Label Gate` MUST require exactly one `type:*` label from the declared type set and exactly one `channel:*` label from the declared channel set. Patch plus stable uses automatic next-patch preparation, advancing past already-owned product tags when necessary; major, minor, and every RC use a controlled exact version; docs and skip do not publish.

Covers: G1, G3, A3.

### REQ-PVR-004: Preparation is a PR-local VERSION-only commit

After all source PR checks succeed, trusted preparation MAY create one single-parent commit on the PR branch using GitHub GraphQL `createCommitOnBranch` and `GITHUB_TOKEN`, guarded by `expectedHeadOid`. The commit MUST change only `VERSION`, include source/version/intent trailers, and have GitHub `commit.verification.verified == true`. No GPG secret, dedicated bot account, or bypass path is part of the contract.

Covers: G3, A3.

### REQ-PVR-005: Release follows normal merge and supports ordered same-identity recovery

Release completion MUST validate source checks, preparation ancestry, merge structure, VERSION, and tag ownership. The normal release workflow reads the committed merge SHA and VERSION, builds and verifies all macOS assets, and creates the immutable tag/release. The release-owning agent MUST report successful publication directly to the owner, and Release Product MUST NOT create or update a result comment on the source PR. Manual dispatch MUST accept only `recover` for the same merge SHA and VERSION. Snapshot, queue, arbitrary SHA backfill, and retagging are forbidden.

Before either automatic publication or `recover`, Release Product MUST enumerate the remote product tags matching `vX.Y.Z` and `vX.Y.Z-rc.N` and compare the candidate with the highest full-SemVer tag. A lower candidate MUST fail before build, tag, or asset work with `superseded_by_product_tag`. An equal candidate MUST prove that the existing tag targets the same merge SHA. A higher candidate is the only candidate eligible for a new tag. If the matching tag already has a draft Release, the workflow MAY replace its assets and MUST publish it explicitly; a published matching Release is an idempotent success and MUST NOT rebuild or overwrite assets.

Covers: G4, A3, A4.

### REQ-PVR-006: Quality and notification contracts are explicit

`.github/quality-gates.json` MUST declare exact required check names and workflow mappings. Source heads run the complete Rust, Swift, and native package matrix; preparation heads run structural fast paths with the same required check names. Eligible failed releases MUST notify with the locked merge/version/tag identity and a same-SHA recovery candidate.

Failure notifications MUST label any command as `recovery_candidate` and include the condition that the current product-tag waterline and same-SHA identity must be rechecked. A superseded candidate MUST state that no Release was created and MUST NOT advertise recovery.

Covers: G5, A5, A6.

## Acceptance evidence

- `scripts/test-product-version.py`
- `.github/scripts/test-release-chain.sh`
- `.github/scripts/test-release-preparation.sh`
- `.github/scripts/test-release-completion.sh`
- `.github/scripts/test-release-workflows.sh`
- `.github/scripts/test-package-scripts.sh`
- `.github/quality-gates.json` checked with the repository quality-gates checker
