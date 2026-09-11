# ADR 0010: Identity-Stable Single-Product Release

## Decision

Ship one visible product bundle, `TelevyBackup.app`. Keep Snapshot Access as an independent
user-session process and Full Disk Access identity, but embed it at
`Contents/Library/LoginItems/TelevyBackup Snapshot Access.app`. Register its LaunchAgent through
`SMAppService.agent` using a bundle-relative `BundleProgram`; no user LaunchAgent plist may contain
an absolute workspace or installation path.

The first RC that introduces this layout builds and ad-hoc signs a Universal Snapshot Access
bundle. Later RCs and the stable release for that product version reuse that exact helper artifact
from `v<product-version>-rc.1`'s Universal DMG. Reuse forbids rebuilding, `lipo`, deep signing,
or any other mutation of the helper.
The outer app is signed from the inside out. The helper's component version, IPC protocol version,
source policy, SHA-256, CodeDirectory hash, and designated requirement are recorded in the release
manifest.

The main app performs a transactional migration from the old external LaunchAgent. It backs up the
old plist and manifest, refuses to switch while a lease is active, unregisters the old service,
registers the embedded agent, confirms the running helper identity, and then commits. Registration
failure restores the old plist and manifest. The old external app is never deleted and TCC/FDA is
never changed by the migration.

The root mount helper remains at its existing privileged system path and is only compatibility
checked in this release. It is not automatically installed, updated, or re-signed as part of a
normal TelevyBackup app update.

All release signatures are ad-hoc. Developer ID, notarization, automatic updates, and TCC
automation are explicitly out of scope.

## Rationale

The old workflow persisted the path supplied to `snapshot-access install --app`, so a developer
workspace path could become the production LaunchAgent target. A top-level helper app also made the
user install two visible applications. A bundle-relative service removes both path sources while
preserving the separate FDA boundary. Reusing unchanged helper bytes is the only reliable way to
avoid changing its ad-hoc TCC identity during an ordinary main-app update.

## Consequences

The first migration requires the user to grant FDA to the embedded helper's new absolute path.
Replacing the main app with a release that reuses the helper does not require another FDA grant.
Actual Snapshot Access code or FDA behavior changes create a new component version and require an
explicit migration and manual FDA review. The tools archive contains CLI and mount-helper tools,
but never a second Snapshot Access app or an install template for it.
