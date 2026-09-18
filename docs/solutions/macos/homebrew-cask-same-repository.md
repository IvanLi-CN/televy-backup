---
title: Same-Repository Homebrew Cask Release
module: macos-release-distribution
problem_type: release-distribution-automation
component:
  - homebrew-cask
  - release-workflow
tags:
  - macos
  - homebrew
  - cask
  - github-actions
  - release-automation
status: active
related_specs:
  - docs/specs/macos-release-distribution/SPEC.md
---

# Same-Repository Homebrew Cask Release

## Context

TelevyBackup has no Apple Developer ID or notarization credentials. Its stable macOS
release is therefore an ad-hoc signed Universal 2 DMG. Homebrew can still provide a
convenient installation surface, but the Cask must preserve the normal macOS quarantine
and Gatekeeper flow and must not pretend to be a notarized distribution.

The product repository is also the Homebrew Tap. A second tap repository would duplicate
the Cask source and require cross-repository credentials, so the repository keeps the GUI
Cask at the tap-root path `Casks/televybackup.rb`. The legacy daemon Formula remains a
separate packaging surface under `packaging/homebrew/` and is not part of the GUI Cask
lifecycle.

The Cask follows only the latest stable `prod` Product Release. Its immutable inputs are
the Universal 2 DMG, `SHA256SUMS`, and `BUILD-MANIFEST.json` published by the same GitHub
Release.

## Symptoms

This problem usually appears as one or more of the following:

- A Cask points at a prerelease, a mutable branch, or an asset from a different release.
- The Cask version and SHA-256 disagree with the published Release metadata.
- Homebrew installs the daemon, a privileged helper, or development app identity along
  with the GUI app.
- An automated update requires a PAT, GitHub App, repository secret, or a second tap
  repository.
- A manually dispatched workflow reports success while the PR's required check remains
  failed.
- A documentation-only Cask PR fails in Release completion with an `unbound variable`
  error even though no product release is being requested.

## Root Cause

The Cask is a release projection, not an independent version source. It is only correct
when its version, URL, SHA-256, bundle identity, and macOS minimum version are derived
from the same immutable Release metadata. Hand-editing any one of those values creates a
split-brain release surface.

The automation also crosses two GitHub trust boundaries:

1. The post-release workflow runs from trusted `main`, consumes only a published stable
   Release, and writes only the same repository's automation branch and PR.
2. The PR checks must validate the exact automation-branch head before the workflow merges
   that head.

`pull_request_target` workflows execute their workflow definition from the base branch.
Consequently, changing a workflow in the same PR does not repair that PR's own required
check. A green `workflow_dispatch` run on the candidate head is useful evidence, but it
does not replace the required check attached to the PR head.

Release completion has both product-release and documentation-only paths. Optional
preparation state must be initialized before the product-release condition, because the
documentation path intentionally has no VERSION preparation record. With `set -u`, a
later guard such as `[[ -n "${prepared_json}" ]]` fails if the optional value was never
initialized. The workflow contract test must keep this initialization before the branch
and must cover the non-product path.

## Resolution

### Repository layout

Keep exactly one GUI Cask definition at the tap root:

```text
Casks/televybackup.rb
```

Do not add a second GUI Cask under `packaging/homebrew/`. Keep the legacy daemon Formula
separate when it is still needed:

```text
packaging/homebrew/televybackupd.rb
```

The Cask installs only `TelevyBackup.app` with the production bundle identity
`com.ivan.televybackup`. It does not install or configure the daemon, snapshot access
helper, mount helper, Full Disk Access permission, configuration, or backup data.

### Render from immutable Release metadata

The renderer rejects prerelease versions, requires a Universal 2 artifact, requires the
versioned DMG in `SHA256SUMS`, and checks that `SHA256SUMS` agrees with
`BUILD-MANIFEST.json`.

Render and verify a candidate with:

```bash
python3 scripts/homebrew/cask_release.py render \
  --version "${VERSION}" \
  --checksums "${RELEASE_DIR}/SHA256SUMS" \
  --manifest "${RELEASE_DIR}/BUILD-MANIFEST.json" \
  --output "${CASK_PATH}"

python3 scripts/homebrew/cask_release.py verify-cask \
  --cask "${CASK_PATH}" \
  --version "${VERSION}" \
  --checksums "${RELEASE_DIR}/SHA256SUMS" \
  --manifest "${RELEASE_DIR}/BUILD-MANIFEST.json"
```

The generated Cask uses the immutable same-repository Release asset:

```text
https://github.com/IvanLi-CN/televy-backup/releases/download/v{version}/TelevyBackup-{version}.dmg
```

### User installation

The repository name does not use Homebrew's conventional `homebrew-` prefix, so users
should provide the explicit tap URL:

```bash
brew tap IvanLi-CN/televy-backup https://github.com/IvanLi-CN/televy-backup.git
brew install --cask IvanLi-CN/televy-backup/televybackup
```

The Cask leaves quarantine intact. Users should verify the downloaded Release asset and
then approve the first launch in Finder through Gatekeeper if macOS asks for approval.
The absence of notarization is an expected property of this distribution path.

### Release-to-Cask automation

The trusted update workflow follows this sequence:

1. Trigger from a successful `Release Product` workflow or a manual retry for a published
   stable tag.
2. Resolve and validate a non-draft, non-prerelease `prod` Release.
3. Download only `SHA256SUMS` and `BUILD-MANIFEST.json` from that Release.
4. Render and verify the Cask candidate.
5. Create or reuse the same-repository `automation/homebrew-cask` branch, and reject
   branch history containing changes outside `Casks/televybackup.rb`.
6. Create or update a PR labelled `type:docs` with no release-channel label.
7. Dispatch the normal required checks plus the read-only Homebrew Cask audit against
   the exact branch head.
8. Poll the PR and check runs while binding every result to the expected repository,
   base branch, PR number, and head SHA.
9. Merge only with a SHA-constrained squash merge after every required check succeeds,
   then delete the automation branch.

The Homebrew audit workflow is read-only. It validates Ruby syntax, runs strict Homebrew
Cask audit, downloads the referenced stable DMG, verifies its checksum and manifest, and
inspects the production bundle identity, minimum macOS version, and both architecture
slices.

### Credential boundary

The update workflow uses GitHub Actions' built-in `GITHUB_TOKEN` for same-repository
branch, PR, label, check-dispatch, and guarded-merge operations. The audit workflow has
read-only `contents` and `pull-requests` permissions. No PAT, GitHub App, fine-grained
token, or repository secret is part of the normal design.

The workflow's write permissions are intentionally confined to the same repository. A
cross-repository tap, a token stored in CI, or a direct write to the default branch would
change the trust model and is not a compatible extension of this solution.

## Guardrails / Reuse Notes

- Stable Casks must use final `X.Y.Z` versions only. Do not publish beta, RC, dev, branch,
  or commit URLs through the stable Cask.
- Keep `Casks/televybackup.rb` generated from Release metadata. Treat a version or digest
  mismatch as a release input error, not as a reason to edit the checksum by hand.
- Preserve quarantine and document Gatekeeper confirmation. Never add a Cask postflight
  step that removes quarantine, requests FDA, runs privileged commands, or manages the
  daemon.
- Keep the Homebrew audit read-only and keep write permissions in the post-release update
  workflow only.
- Validate the exact PR head immediately before merge. A successful check from another
  head or a successful manual dispatch is not sufficient evidence.
- When Release completion fails on a Cask PR, check the following in order:
  1. Confirm the PR is open, targets `main`, belongs to the same repository, and still has
     the expected head SHA.
  2. Identify whether the failing workflow is `pull_request_target`; if so, inspect the
     trusted workflow from `main`, not only the candidate branch.
  3. Check whether a documentation-only path reads an optional release-preparation value
     under `set -u`.
  4. Run the release workflow contract test and the Homebrew Cask contract test after the
     fix.
  5. Re-run the required PR checks for the current head. Do not reinterpret a manual
     dispatch result as the PR's required check.
- Keep the optional release-preparation variables initialized before their product-release
  condition. Add a contract assertion for both initialization and the non-product path
  whenever the release workflow is refactored.
- Do not make branch-protection bypasses part of the normal Cask update workflow. A
  temporary exception is an owner-controlled incident action, not a release mechanism.

## Validation

Run the repository-level contracts from a clean checkout:

```bash
bash .github/scripts/test-homebrew-cask.sh
bash .github/scripts/test-release-workflows.sh
```

On macOS, the audit workflow additionally runs:

```bash
brew audit --cask --strict --tap=IvanLi-CN/televy-backup
```

The downloaded asset must pass the renderer's `verify-dmg` checks before the Cask PR is
merged. Those checks bind the DMG filename and SHA-256 to the stable Release metadata,
verify the DMG, and inspect the app bundle's production identity and architecture set.

## References

- [macOS release distribution specification](../../specs/macos-release-distribution/SPEC.md)
- [Homebrew Cask ADR](../../adr/0015-single-repository-homebrew-cask.md)
- [Cask definition](../../../Casks/televybackup.rb)
- [Cask renderer and verifier](../../../scripts/homebrew/cask_release.py)
- [Read-only Cask audit workflow](../../../.github/workflows/homebrew-cask.yml)
- [Stable Cask update workflow](../../../.github/workflows/homebrew-cask-update.yml)
- [Homebrew Cask contract test](../../../.github/scripts/test-homebrew-cask.sh)
- [Release workflow contract test](../../../.github/scripts/test-release-workflows.sh)
