# ADR 0016: Direct Launchctl Snapshot Access Registration

## Status

Accepted. This ADR supersedes the official ad-hoc registration portions of ADR 0010 and ADR 0011.

## Context

The official product uses direct `launchctl bootstrap` because it is signed ad-hoc and cannot
depend on `SMAppService`. macOS `launchd.plist(5)` supports `BundleProgram` only for plists
installed by `SMAppService`. A direct bootstrap of the embedded `BundleProgram` plist therefore
fails with `Bootstrap failed: 5: Input/output error`, leaving the Snapshot Access service absent
and its stale socket unreachable.

## Decision

The embedded Snapshot Access LaunchAgent plist for official ad-hoc releases uses `Program` with the
fixed canonical executable path inside `/Applications/TelevyBackup.app`. Production migration and
registration remain rejected from every other app location, so neither a workspace path nor a
user-supplied helper path can become the registered executable.

The main app continues to bootstrap that product-bundled plist through `launchctl` and keeps the
transactional prepare, identity check, commit, and rollback flow. Separately signed builds may use
`SMAppService` and its `BundleProgram` support instead. The helper remains embedded and retains its
independent FDA identity; this changes only how launchd locates its executable.

## Rationale

The canonical product path is required before migration and is stable across ordinary app updates.
It provides a direct-launchctl format macOS accepts without reintroducing a configurable external
LaunchAgent or a second visible app. An isolated unique-label launchctl fixture proves that the
`Program` form registers and unloads successfully without starting the Snapshot Access helper.

## Consequences

Release verification rejects `BundleProgram` in the official ad-hoc plist and checks the exact
canonical `Program` value. The package matrix runs the isolated bootstrap fixture on both native
macOS architectures. Existing manifest migration and Full Disk Access grants remain unchanged.
