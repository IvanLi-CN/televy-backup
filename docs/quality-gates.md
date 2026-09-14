# Quality gates

TelevyBackup treats pull request checks as an explicit merge contract. The canonical declaration is `.github/quality-gates.json`; this document explains the repository-facing policy without changing GitHub settings.

## Required checks

- `quality`
- `macOS Swift tests`
- `arm64 native package`
- `x86_64 native package`
- `Universal 2 assembly`
- `Release intent label gate`
- `Release completion`

The exact workflow mapping is declared in `.github/quality-gates.json` and is validated by the style-topic quality-gates checker. The preparation classifier jobs are intentionally informational helpers and are not required checks.

Release Product treats the highest eligible final product tag as the numeric baseline. Prerelease
ordinals are allocated only within their base/channel. Reservation, bound, and consumed refs are
append-only; any provenance or ownership conflict fails before packaging. A matching published
Release is terminal and is not rebuilt or overwritten. Failure alerts use `recovery_candidate` only
after a complete merged identity is rechecked.

## Release checks

`Label Gate` enforces exactly one product `type:*` and one `channel:prod|beta|rc|dev` label, or a
channel-free `type:docs|skip` intent. Source PR heads run the full Rust, Swift, and native package
matrix. A trusted preparation run reserves identity, adds only `VERSION` to the PR branch, and then
the same required check names run structural verification against that preparation commit.
`Release completion` is the required PR-local contract for ancestry, VERSION, labels, source checks,
reservation provenance, and the explicit version-only release PR mode.

Release Product resolves Snapshot Access helper state separately from the product RC ordinal. It
reuses only a published prerelease Release whose Universal artifact, manifest, checksums, and helper
identity match the component lock. The resolve job binds the exact Universal DMG, manifest, and
checksums into the immutable `snapshot-helper-source` workflow artifact; native build and assembly
jobs consume that artifact and never redownload helper assets from the Release. Invalid candidates
fall back to the next candidate. If no approved helper Release exists, a same-SHA recovery must
explicitly set `helper_mode=bootstrap`; the resulting bootstrap preserves the existing reservation
and merge identity. A published product Release or consumed receipt is terminal before helper
resolution and never re-enters packaging. The package-ci development artifact is not an approved
Release source.

After a normal merge, `Release Product` reads only the committed merged identity and its reservation
ref. Its manual entry is restricted to same-identity `recover`. Successful publication is reported
directly to the owner by the release-owning agent; Release Product does not write a result comment to
the source PR. Failed releases are handled by `Notify failed release`, which reports locked identity
context only when it can be resolved.
Successful publication is reported directly to the owner by the release-owning agent; Release Product does not write a result comment to the source PR.

## Remote alignment

The declaration is the repository source of truth. GitHub labels, required checks, signed commits,
main branch rules, and reservation/receipt/product tag protection are reconciled only at PR-ready
Step 5C by the release-owning agent. Unrelated rules are preserved and insufficient permissions
remain an explicit blocker.

## Local verification

Run `bash .github/scripts/test-release-scripts.sh`, the focused release fixture scripts, `bash .github/scripts/test-package-scripts.sh`, and the Rust checks before opening a PR. Hosted macOS jobs remain authoritative for Swift and native packaging.
