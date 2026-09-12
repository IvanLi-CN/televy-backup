# ADR 0010: Identity-Stable Single-Product Release

## Decision

Ship one visible product bundle, `TelevyBackup.app`. Keep Snapshot Access as an independent
user-session process and Full Disk Access identity, but embed it at
`Contents/Library/LoginItems/TelevyBackup Snapshot Access.app`. Register its LaunchAgent through
`SMAppService.agent` using a bundle-relative `BundleProgram`; no user LaunchAgent plist may contain
an absolute workspace or installation path.

The first RC that introduces this layout builds and ad-hoc signs a Universal Snapshot Access
bundle. Later RCs and the stable release reuse the approved helper artifact from the current
product-version RC1; ordinary future product versions reuse the last approved RC1 artifact across
product-version boundaries. The `bootstrap_release_tag` in
`packaging/macos/snapshot-components.lock.json` names that artifact. It changes only when the
Snapshot Access component or its FDA behavior changes, which makes that version's RC1 the new
explicit authorization migration point. Reuse forbids rebuilding, `lipo`, deep signing, or any
other mutation of the helper.
The outer app is signed from the inside out. The helper's component version, IPC protocol version,
source policy, executable SHA-256, complete artifact digest, CodeDirectory hash, and designated
requirement are recorded in the release manifest.

The main app performs a transactional migration from the old external LaunchAgent. It backs up the
old plist and manifest, refuses to switch while a lease is active, unregisters the old service,
registers the embedded agent, confirms the running helper identity, and then commits. A short-lived
owner token prevents another app instance from committing or rolling back the transaction.
Registration failure restores the old plist and manifest. The old external app is never deleted and
TCC/FDA is never changed by the migration.

The root mount helper remains at its existing privileged system path and is only compatibility
checked in this release. Its release manifest entry is a bundled compatibility reference, while
the installed path and identity are confirmed by the manual RC acceptance evidence. It is not
automatically installed, updated, or re-signed as part of a normal TelevyBackup app update.

Development variants and custom config/data directory runs use the same embedded helper as a
process-local child with explicit environment variables. They do not register the production
SMAppService agent because its bundle-relative plist intentionally has no mutable directory
configuration.

All release signatures are ad-hoc. Developer ID, notarization, automatic updates, and TCC
automation are explicitly out of scope.

## Rationale

The old workflow persisted the path supplied to `snapshot-access install --app`, so a developer
workspace path could become the production LaunchAgent target. A top-level helper app also made the
user install two visible applications. A bundle-relative service removes both path sources while
preserving the separate FDA boundary. Reusing unchanged helper bytes is the only reliable way to
avoid changing its ad-hoc TCC identity during an ordinary main-app update.

## Consequences

The first migration requires the user to grant FDA to the embedded helper's new absolute path and
must be run from `/Applications/TelevyBackup.app`, never from a mounted DMG. Replacing the main
app with a release that reuses the helper does not require another FDA grant.
Actual Snapshot Access code or FDA behavior changes create a new component version and require an
explicit migration and manual FDA review. The tools archive contains CLI and mount-helper tools,
but never a second Snapshot Access app or an install template for it.
