# Quality gates

TelevyBackup treats pull request checks as an explicit merge contract. The canonical declaration is `.github/quality-gates.json`; this document explains the repository-facing policy without changing GitHub settings.

## Required checks

- `quality`
- `macOS Swift tests`
- `arm64 native package`
- `x86_64 native package`
- `Universal 2 assembly`
- `Validate PR labels`
- `Release completion`

The exact workflow mapping is declared in `.github/quality-gates.json` and is validated by the style-topic quality-gates checker. The preparation classifier jobs are intentionally informational helpers and are not required checks.

## Release checks

`Label Gate` enforces exactly one `type:*` and one `channel:*` label. Source PR heads run the full Rust, Swift, and native package matrix. A trusted preparation run adds only `VERSION` to the PR branch and then the same required check names run structural verification against that preparation commit. `Release completion` is the required PR-local contract for ancestry, VERSION, labels, source checks, and migration handling.

After a normal merge, `Release Product` reads only the committed merge SHA and VERSION. Its manual entry is restricted to same-identity `recover`. Failed releases are handled by `Notify failed release`, which reports the resolved SHA, VERSION, tag, and recovery command.

## Remote alignment

The declaration is the repository source of truth. GitHub ruleset and branch-protection settings must be reconciled separately by an authorized owner; this change records the expected policy but performs no remote mutation.

## Local verification

Run `bash .github/scripts/test-release-scripts.sh`, the focused release fixture scripts, `bash .github/scripts/test-package-scripts.sh`, and the Rust checks before opening a PR. Hosted macOS jobs remain authoritative for Swift and native packaging.
