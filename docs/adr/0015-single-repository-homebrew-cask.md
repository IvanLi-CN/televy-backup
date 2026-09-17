# ADR 0015: Homebrew Cask in the Product Repository

TelevyBackup's existing public repository also serves as its Homebrew tap, with the GUI Cask at the tap-root `Casks/televybackup.rb`; no second tap repository or mirrored Cask source is maintained. Stable releases use the Universal 2 DMG, and a trusted same-repository workflow uses GitHub's built-in `GITHUB_TOKEN` to create the Cask PR, dispatch its checks, and merge it after the exact head passes. No PAT, GitHub App, fine-grained token, or repository secret is required. The Cask installs only `TelevyBackup.app`, leaves quarantine intact, and does not manage daemon or privileged-helper state because releases are ad-hoc signed and not notarized.

## Consequences

- Homebrew users tap `IvanLi-CN/televy-backup` with the explicit GitHub repository URL because its repository name does not use Homebrew's conventional `homebrew-` prefix.
- The post-release workflow generates the Cask from the published `SHA256SUMS` and `BUILD-MANIFEST.json`, creates or updates a same-repository PR, dispatches its checks, and merges only after every exact-head check succeeds.
- The audit workflow is read-only; only the post-release trusted workflow has the minimum same-repository write permissions needed for its branch, PR, labels, check dispatches, and guarded merge.
- The historical daemon Formula remains separate and is not part of the GUI Cask lifecycle.
