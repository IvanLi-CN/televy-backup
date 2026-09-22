# ADR 0011: Ad-hoc Embedded Agent Registration

## Status

Superseded by ADR 0016 for the official ad-hoc registration backend. This ADR remains the
historical record of the original `BundleProgram` decision; ADR 0010 remains the authority for
single-product packaging and component identity reuse.

## Decision

Official TelevyBackup releases are signed ad-hoc and do not depend on Developer ID, notarization, or
TCC automation. The production app registers the embedded Snapshot Access LaunchAgent with
`launchctl bootstrap gui/<uid>`, passing the plist stored at
`TelevyBackup.app/Contents/Library/LaunchAgents/com.ivan.televybackup.snapshot-access.plist`.
That plist uses only the bundle-relative `BundleProgram` and contains no absolute workspace or
installation path.

The launch service remains owned by `launchd` after the GUI exits, so CLI-only scheduled backups
continue to work. The main app uses the same transactional prepare, registration, identity check,
commit, and rollback sequence for this backend. Rollback boots out the embedded service before
restoring the old external registration.

`SMAppService.agent` remains an optional backend for separately signed builds, but it is not used or
required by the official ad-hoc release contract. The registration manifest accepts both backend
values so a signed build can remain compatible with an existing installation; official ad-hoc
migrations record `managedBy=launchctl-embedded`.

## Rationale

`SMAppService` is the preferred API for a normally signed macOS product, but this project has no
Developer ID distribution identity and the official release path cannot depend on that backend.
Directly spawning the helper from the GUI would make the helper's lifetime depend on the GUI and
break scheduled CLI operation. Bootstrapping the embedded
bundle-relative plist with `launchctl` preserves launchd ownership, avoids a second user-visible app,
and does not create or persist a user-controlled absolute helper path.

## Consequences

Ad-hoc releases must be installed at `/Applications/TelevyBackup.app` before production migration;
the embedded plist is never bootstrapped from a mounted DMG. The first migration still requires a
manual FDA grant for the embedded Snapshot Access app. Replacing the main app at the same path with
the exact unchanged helper artifact does not require another FDA grant. A Snapshot Access code or FDA
behavior change creates a new component version and an explicit migration.
