# History

- 清理恢复审查补齐删除后 inventory 漂移的重试：提前清空归属 manifest 会将空 root 的 pending 行误归为未完成 reservation，保留归属直到 released 提交可避免该死锁。混合 pending+drift 的标准升级同时对齐 CLI 与 UI 的有效旧 build identity 门禁。

- 快照源读取与远端上传曾共享内存队列，Telegram 背压会延长源 lease；同时 cleanup 要求旧 baseline 完全一致，外部 inventory 变化导致 pending 与 released drift 无法按序收敛。修复将加密 pack 暂存与上传分离，并在保留 journal 归属边界的前提下先清理、后重建 released baseline。稳定 Snapshot Access FDA 组件的产品版本差异不是本次故障归因。

- Maintenance UI could report failure after 60 pending/running polls while the daemon continued reconciliation. The status callback also admitted out-of-order replies without a request fence. UI observation now follows the daemon terminal state, preserves accepted operation IDs when a query disconnects, and applies only the latest status response. A synthetic 72-to-zero reply-order regression covers stale overwrites; it does not establish that this race was the sole cause of a particular owner screenshot.

- The first legacy-repair release could leave every old row blocked when the journal omitted its
  reservation baseline, the APFS device node had been renumbered, or the recorded mount directory
  had already disappeared. The `0.5.5` helper resolves the volume by UUID, matches inventory by
  snapshot UUID, migrates empty reservations without deleting unrecorded snapshots, and cleans a
  root-owned legacy path only after proving it is not mounted. The hosted fixture seeds 65 such
  rows and requires all 65 to be audited, migrated, released, and absent from the final status;
  ambiguous mounted or malformed rows remain fail-closed.

- The hosted recovery fixture no longer assumes that a disposable APFS image can create a local
  snapshot. GitHub-hosted macOS rejects `tmutil localsnapshot` for that image and `diskutil apfs`
  has no snapshot-create operation. The fixture now proves the safer boundary: an unobservable
  legacy and reservation manifests transition only to the verified `pending_cleanup` fence;
  daemon and CLI repair remain blocked. Successful journal-bound release semantics stay in
  deterministic helper tests. No CI result claims a snapshot was created or deleted without
  inventory evidence.

- Legacy root-helper rows that predated persisted mount-device identity could be reported as
  unknown blockers indefinitely: ordinary Refresh, helper Update and stale cleanup correctly
  refused to guess whether a mount was still live, but no user-invoked repair path existed. The
  `0.5.4` helper now exposes an explicit `snapshot.mount_helper.repair_legacy` operation. It is
  serialized with Snapshot Access, age-gated, peer-UID scoped and restricted to root-owned managed
  mount roots with valid volume/device bindings, manifests and inventory; eligible rows are
  backfilled and passed through the existing `Release` boundary, while every ambiguous row remains
  blocking. The CLI and Settings surface counts and a repair action without exposing paths,
  identifiers or source bytes. Protocol v4 and the approved Snapshot Access artifact remain
  unchanged.

- Installer startup could be incorrectly reported as failed because four immediately refused connections exhausted the readiness check before initialization completed. Readiness now uses a monotonic startup window, with regression coverage for delayed initialization, permanent refusal, stale identity and late replies. Mount permission failures retain their fixed operation category for presentation.
- [PR #181](https://github.com/IvanLi-CN/televy-backup/pull/181) includes a [hosted macOS installer fixture](https://github.com/IvanLi-CN/televy-backup/actions/runs/38865387153/job/116856982379) that delays real journal initialization with a disposable SQLite lock and verifies installation reaches the candidate build without a manual refresh. The source candidate's installer and strict error-propagation checks pass; this fixture does not exercise real FDA or establish full backup completion.

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
