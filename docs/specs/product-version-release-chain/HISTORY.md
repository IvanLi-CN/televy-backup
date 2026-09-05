# History

- Product numeric identity moved from tags, Cargo metadata, and release environment fallbacks to the root `VERSION` contract.
- The previous release-intent freeze, snapshot, queue, and exact-tag backfill paths were replaced by a PR-local VERSION-only preparation commit and normal-merge completion.
- GitHub-native verified commits through `createCommitOnBranch` replaced repository-managed signing secrets; no GPG private key or passphrase is required.
- Automatic patch preparation skips historical product tags already owned by another commit while preserving the patch/stable release intent.
