# History

- Installer startup could be incorrectly reported as failed because four immediately refused connections exhausted the readiness check before initialization completed. Readiness now uses a monotonic startup window, with regression coverage for delayed initialization, permanent refusal, stale identity and late replies. Mount permission failures retain their fixed operation category for presentation.

- Helper update readiness: a reachable root helper with the same compatibility version could remain on an older build while Settings displayed a green installed state and hid its installation action. The daemon now obtains build evidence directly from root Status, and Settings compares it with the current app before declaring readiness. Older daemon responses remain decodable but show an update action. Installation remains explicit and administrator-authorized; neither root protocol nor the approved Access artifact changes.

- Source reservation repair: hosted macOS reproduces the nested-directory `diskutil info -plist` exit 1 through the unchanged production Reserve path. Source validation now queries the pinned directory's kernel device, preserves UID and volume identity checks, and retains privacy-safe command diagnostics. Installation distinguishes source builds through additive Status identity and binary hashes while keeping the existing compatibility version, protocol, journal and approved Access artifact.
- [PR #179](https://github.com/IvanLi-CN/televy-backup/pull/179) records the [baseline production reproduction](https://github.com/IvanLi-CN/televy-backup/actions/runs/38737087429) and the [passing source-candidate regression](https://github.com/IvanLi-CN/televy-backup/actions/runs/38740114413). The regression covers real Reserve, denied identities and peers, loss of a disposable APFS mount, and candidate installation identity while preserving the approved Access artifact. The prepared product version remains governed by the release-chain Spec; these checks prove source preparation and installation contracts, not full backup completion.

## Surviving Access process upgrades

- Published rc.114 and rc.120 Access artifacts both advertised component version `0.2.0` despite expecting mount protocols 1 and 4. On a disposable Actions runner, updating files at the same paths and successfully installing the new root helper left the old Access process alive. Direct LaunchAgents omit parent bundle metadata, and duplicate bootstrap returned error 5 without the text previously required for restart recovery. Running-code validity now distinguishes the old process, registration refreshes only an idle owned service, and the installation coordinator includes Access registration before reporting communication readiness. The approved Access artifact remains unchanged, preserving its FDA identity. Registration is serialized across application instances, and the hosted fixture proves service shutdown before deleting its created state.
- [PR #177](https://github.com/IvanLi-CN/televy-backup/pull/177) fixes the surviving-process upgrade. The [hosted regression](https://github.com/IvanLi-CN/televy-backup/actions/runs/38612289981) first reproduces the published protocol failure, then verifies candidate production code rejects the old running identity, replaces the idle process, restores communication, retains the healthy replacement on a repeat registration, and removes every fixture service and created privileged state.

## Installation feedback

- PR #176 follows the protocol-compatibility release with a hosted regression for the privileged installer and its UI feedback. A status refresh previously overwrote administrator installation failures; installation and Access communication now form one completion result, and the CLI verifies the new service before acknowledging readiness. The reproduced feedback failure and the passing published-artifact installation are distinct evidence; neither establishes the cause of an owner's inaccessible runtime state.

## Durable Rationale

- APFS local snapshots are used as a read-only filesystem view because live-file scanning cannot guarantee one backup time point.
- The privileged component remains a narrow mount and lifecycle proxy so user Keychain and backup data stay in the user daemon.
- The file-reading process remains an independent FDA identity, but is delivered inside the single
  visible product app. Official ad-hoc registration uses the canonical product `Program` path because
  macOS reserves `BundleProgram` for `SMAppService`; its unchanged artifact is reused across ordinary
  releases, while helper code changes remain explicit authorization migrations.
