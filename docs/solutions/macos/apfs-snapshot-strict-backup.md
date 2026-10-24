---
title: Strict APFS Snapshot Backup Without Exposing User Data
module: macos-backup
problem_type: data-consistency-and-privilege-boundary
component:
  - snapshot-access
  - snapshot-mount-helper
  - backup-daemon
tags:
  - macos
  - apfs
  - snapshot
  - full-disk-access
  - privacy
  - least-privilege
status: active
related_specs:
  - docs/specs/apfs-snapshot-consistency/SPEC.md
---

# Strict APFS Snapshot Backup Without Exposing User Data

## Context

A directory backup that walks live files cannot promise that every file came from one point in
time. APFS local snapshots provide a read-only timepoint, but macOS separates three authorities:

- creating and reading files protected by TCC / Full Disk Access (FDA),
- mounting an APFS snapshot, and
- encrypting, indexing, and uploading backup data.

Combining those authorities in a root backup daemon would make the blast radius unnecessarily
large. Copying the entire source tree to a staging directory avoids the mount boundary but is not
acceptable for large sources and still does not replace application-level transaction semantics.

The solution is a brokered, fail-closed snapshot reader: a user-session app owns snapshot file
reads, a minimal root service owns only the mount lifecycle, and the ordinary backup daemon receives
only directory entries and bounded byte streams.

## Locked Design

```text
normal GUI / CLI / daemon
  - scheduling, Keychain, encryption, chunking, upload, indexes
  - no root privilege
  - no snapshot mount path

Snapshot Access.app (logged-in user)
  - FDA boundary and configured-target allowlist
  - unique snapshot discovery and user-owned lease journal
  - scans and opens files only below the private snapshot mount
  - IPC: metadata pages and bounded read streams

Snapshot Mount Helper (root LaunchDaemon)
  - Reserve, RecordCleanup, Mount, Release, Status, volume-scoped StatusVolume, journal-bound Cleanup only; Reserve owns the source volume before snapshot creation, RecordCleanup invokes tmutil inside the serialized mutation section and journals the complete post-reservation set, and age-gated recovery releases only an unchanged baseline while leaving unknown snapshots blocked
  - mount_apfs, umount, and exact recorded UUID deletion
  - no source file opens, Keychain, encryption, indexes, or network access
```

The daemon retains the logical source path for backup history and manifests. It does not receive the
physical mount path. A strict backup uses a configured target ID to acquire an opaque lease, then
uses that lease for scan and stream requests. A lease is released as soon as source bytes have been
queued for encryption/upload; the remaining pipeline has no snapshot dependency.

```mermaid
sequenceDiagram
    participant D as GUI / CLI / daemon
    participant A as Snapshot Access
    participant H as Root mount helper
    participant V as APFS volume
    participant S as Remote storage

    D->>A: AcquireLease(configured target ID)
    A->>H: Reserve(source identity and UID)
    A->>H: RecordCleanup(lease)
    H->>V: tmutil localsnapshot and inventory checks
    H-->>A: journaled snapshot UUID
    A->>H: Mount(recorded UUID, private directory)
    H->>V: mount_apfs -s snapshot
    D->>A: ScanPage and OpenReadStream(lease, relative path)
    A-->>D: bounded snapshot metadata / bytes
    D->>S: encrypt and upload
    D->>A: ReleaseLease
    A->>H: unmount and UUID-scoped cleanup
```

## Nested source reservation failures

`diskutil info -plist` accepts a volume or device, not an arbitrary nested directory. Hosted macOS reproduces a nested-directory query exiting 1 with empty stderr, while querying the enclosing device succeeds. In the old Reserve path this became a blank `command_failed` and surfaced as `snapshot.acquire_failed`. Resolve the device with `fstatfs` on a pinned no-follow directory metadata descriptor, then validate APFS type, device and UUID through a device query. Bind the non-root authorization child's opened directory to the same device/inode and recheck pathname identity before recording the reservation. Never treat an identity/query failure as an empty inventory or permit live-source fallback.

System tools may report their useful diagnostic on stdout even when stderr is empty. Preserve the fixed operation stage, command category, exit code and selected diagnostic channel; project output onto known safe error classes before limiting its length. Do not echo command arguments, raw plist parser errors, paths, volume UUIDs or arbitrary tool output. Keep the wire compatibility version separate from the component version and build identity: additive `component_version` plus `build_commit` and an installed-file hash prevents a surviving older build with the same wire version from proving readiness while preserving the byte-identical approved Access artifact. Use `CI (main)`'s default-off `snapshot_source_validation` to reproduce and validate with disposable storage; it requires no owner files or TCC changes and does not prove a full remote backup.

## Helper build readiness after an app update

Updating the app does not replace an administrator-installed root helper. A compatibility version such as `0.5.0` can represent multiple source builds. A green socket/protocol check can therefore hide a stale executable and, if the update action is hidden on reachability, leave no way to apply the repair from Settings. Query root Status directly from the daemon because a byte-identical approved Access artifact may not relay newer identity fields. Keep connectivity separate from readiness, compare the root build with both the daemon and current GUI bundle, and keep an explicit update action for missing or mismatched build evidence. Apply the same predicate after installation; the install transaction additionally verifies the installed file hash. Refreshing status must remain read-only. Synthetic old/current/missing states and a mutation of the old connectivity-only predicate provide a fast hosted regression without owner data or TCC changes.

Root Status also reports `pending_cleanup` separately from its legacy `active_mounts` aggregate. Both states remain blocking, but a pending cleanup record is presented as a fail-closed inventory reconciliation issue rather than an active mount. Snapshot acquisition diagnostics classify tombstone and inventory-drift errors as `cleanup_pending` so the UI can explain why a volume is blocked without exposing paths, UUIDs, or raw system output. Status fixtures that model a known active mount set `pending_cleanup` to zero; omitting it remains the deliberate legacy-unknown case.

The helper now reports four blocking categories: recent live rows, pending cleanup, age-gated stale rows whose managed root, manifest, and recorded mount-device identity are structurally valid, and unknown rows. A stale-record recovery button is available only for the third category when the legacy aggregate exactly matches the complete classification. The daemon serializes reconciliation with lease acquisition, requires the current helper component and build identity, verifies that the legacy aggregate equals the complete additive partition and candidate count, confirms Snapshot Access has no active or pending lease, queries the authenticated UID's complete helper status so disabled or removed settings cannot hide a blocker, calls the journal-bound release method, applies the same validation before and after releasing, and leaves pending, live, unknown, inconsistent, and identity-mismatched rows untouched. Missing additive fields remain unknown for readiness and reconciliation, so an older helper cannot be treated as safe by default; the installer may still replace a legacy helper whose required aggregate explicitly reports zero blocking rows, and the replacement must then pass the current component, commit, and file-hash checks. A deletion that completed before a process crash converges through an exact reservation-baseline tombstone retry; an unreachable helper keeps its administrator-authorized install action available. Settings waits for the reconcile operation to reach a terminal state or a bounded timeout before refreshing its read-only status.

Count startup time with a monotonic deadline rather than multiplying a retry count by the status read timeout: a refused Unix connection returns before any read timeout applies. Four probes with 250ms between them can therefore reject a healthy service in less than a second. Keep polling during a bounded startup window, charge probe duration to the same window, preserve the final error, and reject stale identities and late replies. Test this with a virtual clock and delay real helper initialization with a disposable CI journal lock; never add a production command override or a runtime delay switch.

Service reachability does not prove that macOS will permit snapshot mounting. A sanitized `stage=snapshot_mount command=mount_apfs` permission denial should retain a fixed mount-specific UI category instead of being flattened into generic file-access failure. Likewise, known Telegram availability errors should retain a Telegram-specific title even without an optional reason. Preserve raw diagnostics only in local logs and render fixed explanations from synthetic data.

## Invariants

1. Strict mode is opt-in by APFS Volume UUID. Non-APFS, insufficient free space, unavailable
   service, ambiguity while identifying the newly-created snapshot, nested mounts, read errors, and
   pending cleanup are blocking conditions.
2. Strict mode never falls back to a live directory scan. A precondition or broker-read failure
   publishes no Backup Snapshot remotely.
3. The Access IPC accepts configured target IDs, leases, and relative paths only. It rejects
   arbitrary source paths, parent traversal, absolute paths, symlink file reads, and cross-UID use.
4. The root helper mounts only a UUID present in the Access app's recorded manifest. It cleans up
   only that manifest; it never deletes a snapshot discovered by name alone.
5. There is at most one active lease and one coalesced pending run per volume. A cleanup-pending
   volume is blocked until its recorded resources are recovered.
6. Snapshot mount directories are created by the root helper below a root-owned per-user parent;
   the requesting user can read the returned lease path but cannot replace its mount target. The
   daemon cannot bypass the Access IPC.

## Permission Model

| Operation | Component | Authority | Normal backup prompt |
| --- | --- | --- | --- |
| Snapshot file reads | Snapshot Access.app | FDA for the exact installed app identity | No |
| Snapshot creation, mount, unmount, UUID cleanup | Root mount helper | root LaunchDaemon plus FDA for the exact helper identity in the currently validated baseline | No |
| Keychain, encryption, indexes, upload | Backup daemon | ordinary user | No |
| Install/update/remove mount helper | installer / CLI | administrator authorization | Yes, only for that transaction |

FDA is tied to an exact application identity and path. An ad-hoc signed replacement can require
the owner to grant FDA again. Strict mode must display a blocking state in that case; it must never
quietly read the live source.

The locked deployment baseline grants FDA to both Snapshot Access and the root mount helper. The
helper's protocol and code path own snapshot creation and mount cleanup, but the validation that established feasibility
gave both exact identities FDA. It is therefore incorrect to market an Access-only FDA requirement
until the same release has passed the timepoint test after the helper's FDA authorization is removed.
That future least-privilege experiment is optional optimization work, not a prerequisite for this
solution. The settings and installation guidance must list every FDA-authorized identity in the
known-good baseline.

## Why the Read Is From the Snapshot

The essential proof is a timepoint mutation, not merely a matching hash of an unchanged file:

```text
write live/<random probe> = PRE
acquire lease: create and mount snapshot
overwrite live/<same probe> = POST
read lease/<same relative probe>

assert snapshot bytes == PRE
assert live bytes     == POST
release lease and delete the probe
```

`PRE` cannot be obtained by opening the live equivalent path after it has been overwritten with
`POST`. The implementation constructs every read root from the lease's private mount root plus the
configured source-relative path, and never from the logical live source path.

## Privacy-Preserving Evidence

Raw integration transcripts are private diagnostic material: they can contain configured source
paths, target IDs, volume identifiers, snapshot UUIDs, mount roots, and base64 test bytes. They must
not be attached to issues, pull requests, chat, or release notes.

Use the integration verifier's sanitized mode when shareable evidence is needed:

```bash
bash scripts/macos/verify-apfs-snapshot-integration.sh --sanitized \
  '<configured-target-id>:<controlled-source-root>'
```

The controlled source root may be an empty directory created for the test on an already configured
APFS target. The verifier creates only one randomly named probe in that root, and the probe contains
only generated `PRE` / `POST` test values. It does not enumerate, hash, upload, or print existing
user files.

On success, the only stdout record is shaped as follows:

```json
{
  "schema": "televybackup.apfs_snapshot_evidence.v1",
  "result": "passed",
  "cases": 1,
  "assertions": {
    "snapshotReadPreMutation": true,
    "liveMutationObserved": true,
    "leasesReleased": true,
    "cleanupComplete": true
  },
  "redacted": [
    "sourcePath",
    "targetId",
    "volumeUuid",
    "snapshotUuid",
    "mountRoot",
    "testBytes",
    "rawIpc"
  ]
}
```

In sanitized mode the raw IPC transcript is placed in a temporary private directory only while the
test runs, then removed on both success and failure. The report intentionally excludes hostnames,
usernames, paths, disk names, serials, snapshot IDs, timestamps, configuration, and backup data.
It makes no network request.

The privacy claim applies to the produced evidence artifact. It does not attempt to defend against
another process already running as the same macOS user, which could inspect process arguments or
user-owned temporary files; that threat model is outside this design's local IPC boundary.

This report is reproducible functional evidence, not independent cryptographic attestation: a JSON
result alone can be fabricated. To review it meaningfully, pair it with the exact source revision,
the verifier source, reproducible build checks, and the unit tests below. Do not publish raw logs to
make a boolean report appear stronger.

## GitHub Actions Boundary

The repository's hosted required checks run deterministic protocol, path-safety, error-propagation, and workflow-contract checks. The `manual-snapshot-regression` job in `.github/workflows/ci-pr.yml` is dispatch-only and runs the named fixture timepoint, mount-health, broker, and workflow tests against an isolated temporary tree without touching user data. Its published-artifact install phase accepts the legacy status shape, while the candidate phase verifies additive component identity after replacement; the recovery fixture is invoked as an explicit multiline command so its required arguments are not folded into one shell word. The fixture assertions produce only boolean outcomes and bounded error context; they do not read or upload user files.

## Residual Risks

The sanitized report contains no personal filesystem data or host topology, but no useful system can
make the implementation's authority risk zero. The root helper is intentionally small because a
defect in a root process is high impact, and FDA granted to that helper is a deployment-level
capability even though the accepted IPC has no file-read method. The mitigations are a fixed command
allowlist, peer-UID checking, source-volume and private-mount validation, journal-bound manifest cleanup, and
the separation that keeps encryption, Keychain material, indexes, and networking out of the helper.

This solution also provides filesystem timepoint consistency, not application transaction
consistency. A database or application that needs a quiesced or transactionally coordinated backup
must provide its own documented backup/export mechanism.

## Evidence Ladder

| Level | What it establishes | Safe artifact |
| --- | --- | --- |
| Protocol/unit tests | Request allowlists, peer UID binding, private mount validation, journal-bound cleanup | Test names and pass/fail counts |
| Brokered core tests | The daemon uses streams rather than a mount path; strict errors fail closed | Test names and pass/fail counts |
| Sanitized APFS integration | `PRE` survives in snapshot while live file is `POST`; cleanup completes | Sanitized JSON assertion report |
| Package verification | One product bundle contains the independent Access helper and the mount-helper artifact | Artifact names, component hashes, and identity fields, if desired |
| Product backup smoke test | Existing upload pipeline can consume brokered bytes | Private local result; never publish target or remote metadata |

The first three levels establish feasibility without disclosing user data or machine topology. The
last level validates the pre-existing upload pipeline and should remain a local owner-controlled
operation because backup history and remote metadata are sensitive.

## Operational Procedure

1. Build the main app and mount-helper from one source revision; embed Snapshot Access inside the
   main app and register its fixed canonical `/Applications/TelevyBackup.app` `Program` path with
   `launchctl` for the official ad-hoc release path. A separately signed build may use
   `SMAppService` and `BundleProgram`.
2. Let the main app migrate the old external registration transactionally. Install the root helper
   through the explicit administrator transaction; do not run the daemon as root.
3. Grant FDA only through System Settings to the exact displayed identity or identities required by
   the validated deployment baseline.
4. Probe each configured target. Enable strict mode only after the target reports APFS support and
   its Volume UUID is persisted.
5. Before a release or when the access-app identity changes, run the sanitized timepoint verifier
   against a controlled configured root. Retain only its sanitized JSON result.
6. Run unit/package verification. A failure in any strict prerequisite blocks strict backup; it is
   not grounds to silently change the architecture or scan live files.

## Guardrails and Reuse Notes

- Do not use `fs_snapshot_*` private/entitlement-gated APIs, a root file-reading daemon, Time
  Machine configuration changes, or a full staging copy as a substitute for this boundary.
- Do not claim that equal content hashes prove snapshot reads. Only a deliberate post-snapshot live
  mutation can distinguish a mounted snapshot from a live path.
- Do not log raw IPC responses in shareable evidence. `LeaseResult` intentionally includes details
  useful for recovery but unsafe for publication.
- Do not expose physical mount paths to the daemon, CLI, GUI, or remote manifests.
- Do not clean by snapshot name alone or use a global "delete all local snapshots" operation.
- Keep per-volume serialization. Snapshot creation is volume-scoped, even when several configured
  roots lie on the same volume.
- Revalidate the exact release artifact after a signing identity change. For ordinary main-app
  updates, compare the embedded helper SHA-256, CodeDirectory hash, and designated requirement;
  this verifies TCC binding without asking the user to re-authorize an unchanged helper.

## References

- [APFS snapshot consistency specification](../../specs/apfs-snapshot-consistency/SPEC.md)
- [Access app ADR](../../adr/0008-apfs-snapshot-access-app.md)
- [Mount helper ADR](../../adr/0009-apfs-snapshot-mount-helper.md)
- [Brokered read root](../../../crates/snapshot-helper/src/main.rs)
- [Root mount command boundary](../../../crates/snapshot-helper/src/mount_helper.rs)
- [Privacy-preserving integration verifier](../../../scripts/macos/verify-apfs-snapshot-integration.sh)
