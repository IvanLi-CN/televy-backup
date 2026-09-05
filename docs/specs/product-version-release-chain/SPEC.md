# TelevyBackup VERSION-only Release Chain

## Status

Implemented in the current release-chain delivery. The repository contract is normative; GitHub ruleset changes remain outside this topic.

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

`Label Gate` MUST require exactly one `type:*` label from the declared type set and exactly one `channel:*` label from the declared channel set. Patch plus stable uses automatic next-patch preparation; major, minor, and every RC use a controlled exact version; docs and skip do not publish.

Covers: G1, G3, A3.

### REQ-PVR-004: Preparation is a PR-local VERSION-only commit

After all source PR checks succeed, trusted preparation MAY create one single-parent commit on the PR branch using GitHub GraphQL `createCommitOnBranch` and `GITHUB_TOKEN`, guarded by `expectedHeadOid`. The commit MUST change only `VERSION`, include source/version/intent trailers, and have GitHub `commit.verification.verified == true`. No GPG secret, dedicated bot account, or bypass path is part of the contract.

Covers: G3, A3.

### REQ-PVR-005: Release follows normal merge and supports same-identity recovery

Release completion MUST validate source checks, preparation ancestry, merge structure, VERSION, and tag ownership. The normal release workflow reads the committed merge SHA and VERSION, builds and verifies all macOS assets, and creates the immutable tag/release. Manual dispatch MUST accept only `recover` for the same merge SHA and VERSION. Snapshot, queue, arbitrary SHA backfill, and retagging are forbidden.

Covers: G4, A3, A4.

### REQ-PVR-006: Quality and notification contracts are explicit

`.github/quality-gates.json` MUST declare exact required check names and workflow mappings. Source heads run the complete Rust, Swift, and native package matrix; preparation heads run structural fast paths with the same required check names. Failed releases MUST notify with the locked merge/version/tag identity and same-SHA recovery command.

Covers: G5, A5, A6.

## Acceptance evidence

- `scripts/test-product-version.py`
- `.github/scripts/test-release-chain.sh`
- `.github/scripts/test-release-preparation.sh`
- `.github/scripts/test-release-completion.sh`
- `.github/scripts/test-release-workflows.sh`
- `.github/scripts/test-package-scripts.sh`
- `.github/quality-gates.json` checked with the repository quality-gates checker
