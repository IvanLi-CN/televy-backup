# ADR 0012: Independent Helper Bootstrap State

## Status

Accepted. This decision refines the helper-source selection in ADR 0010 without changing its
ad-hoc signing or byte-identical reuse requirements.

## Decision

Product RC ordinals and Snapshot Access helper bootstrap state are independent identities. Release
Product first resolves a published prerelease Release whose Universal DMG, `BUILD-MANIFEST.json`,
`SHA256SUMS`, component metadata, SHA-256, CodeDirectory hash, and designated requirement match
the checked-in component lock. It prefers the current product RC1, then the locked bootstrap tag,
then discovers other published prerelease Releases. A missing or stale preferred tag does not
change the product version or allocate a successor.

When no approved helper Release can be proven, only an explicit same-SHA recovery dispatched with
`helper_mode=bootstrap` may perform the one-time Universal helper build. The resulting release
keeps the existing merge SHA, version, reservation, and bound receipt. Every later ordinary
release reuses the approved helper bundle's original bytes without rebuilding, `lipo`, re-signing,
or deep-signing it. Stable publication still requires a previously published RC helper.

The surviving package-ci development artifact is not an approved helper source: matching component
metadata alone cannot replace a published Release and its release-bound provenance.
