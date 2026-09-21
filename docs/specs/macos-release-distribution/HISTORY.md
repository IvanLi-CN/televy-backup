# History

- The repository previously created GitHub Release records without uploading build assets. This topic makes artifact validation a prerequisite for a public release.
- `v0.9.0` remains an exact-source compatibility backfill. Product-managed service behavior starts with the next patch release.
- The legacy Homebrew daemon Formula remains a compatibility integration; the GUI Cask is maintained at the root of this repository's tap surface and does not own the daemon or LaunchAgent.
- Snapshot Access is a private embedded component rather than a second user-installable app. RC
  and stable packaging reuses its locked Universal artifact when the component has not changed.
- Direct ad-hoc registration uses the fixed canonical product `Program` path because macOS accepts
  `BundleProgram` only for `SMAppService`-installed plists.
- Helper source selection is independent from product RC numbering. A missing approved helper Release
  has an explicit same-SHA bootstrap recovery path; ordinary releases continue to reuse verified
  helper bytes unchanged.
- Helper source assets are bound once in the resolve job and passed to macOS build/assembly jobs as
  the immutable `snapshot-helper-source` workflow artifact; invalid candidates fall back, while
  published and consumed release identities terminate before helper resolution.
