# Legacy Plan 资源索引

所有 legacy Plan 的 canonical 规范已进入 `docs/specs/**`。本目录暂时保留 Plan 与 contracts，直到完成完整映射审查并取得单独的删除确认。

## Plan -> Canonical Spec

| Legacy source | Canonical Spec | Lifecycle | Deletion |
| --- | --- | --- | --- |
| `docs/plan/nvr79:avoid-keychain-in-dev/PLAN.md` | `docs/specs/avoid-keychain-in-dev/SPEC.md` | active | pending delete approval |
| `docs/plan/7r6p4:background-backup-adaptive-throughput/PLAN.md` | `docs/specs/background-backup-adaptive-throughput/SPEC.md` | active | pending delete approval |
| `docs/plan/0007:background-pack-uploads/PLAN.md` | `docs/specs/background-pack-uploads/SPEC.md` | active | pending delete approval |
| `docs/plan/0006:chunking-max-bytes/PLAN.md` | `docs/specs/chunking-max-bytes/SPEC.md` | active | pending delete approval |
| `docs/plan/fn4ny:config-bundle-export-import/PLAN.md` | `docs/specs/config-bundle-export-import/SPEC.md` | active | pending delete approval |
| `docs/plan/0011:daemon-status-ipc/PLAN.md` | `docs/specs/daemon-status-ipc/SPEC.md` | active | pending delete approval |
| `docs/plan/r6ceq:endpoint-scoped-index-chat-uniqueness/PLAN.md` | `docs/specs/endpoint-scoped-index-chat-uniqueness/SPEC.md` | active | pending delete approval |
| `docs/plan/0009:endpoints-settings-page/PLAN.md` | `docs/specs/endpoints-settings-page/SPEC.md` | active | pending delete approval |
| `docs/plan/fwwqp:events-live-task-ui/PLAN.md` | `docs/specs/events-live-task-ui/SPEC.md` | active | pending delete approval |
| `docs/plan/kpmqp:fix-daemon-ipc-sockets/PLAN.md` | `docs/specs/fix-daemon-ipc-sockets/SPEC.md` | active | pending delete approval |
| `docs/plan/fh5ac:fix-upload-rate-display/PLAN.md` | `docs/specs/fix-upload-rate-display/SPEC.md` | active | pending delete approval |
| `docs/plan/w2k9p:import-bundle-rebind-remote-compare/PLAN.md` | `docs/specs/import-bundle-rebind-remote-compare/SPEC.md` | active | pending delete approval |
| `docs/plan/dxddw:import-bundle-target-change-directory/PLAN.md` | `docs/specs/import-bundle-target-change-directory/SPEC.md` | active | pending delete approval |
| `docs/plan/7f9wg:integrate-developer-diagnostics/PLAN.md` | `docs/specs/integrate-developer-diagnostics/SPEC.md` | active | pending delete approval |
| `docs/plan/3ejpg:macos-dev-app-variant/PLAN.md` | `docs/specs/macos-dev-app-variant/SPEC.md` | active | pending delete approval |
| `docs/plan/4fexy:master-key-rotation/PLAN.md` | `docs/specs/master-key-rotation/SPEC.md` | active | pending delete approval |
| `docs/plan/0013:mtproto-dialogs-picker/PLAN.md` | `docs/specs/mtproto-dialogs-picker/SPEC.md` | active | pending delete approval |
| `docs/plan/njr29:mtproto-upload-resilience/PLAN.md` | `docs/specs/mtproto-upload-resilience/SPEC.md` | active | pending delete approval |
| `docs/plan/0005:multi-backup-directories-keyed-restore/PLAN.md` | `docs/specs/multi-backup-directories-keyed-restore/SPEC.md` | active | pending delete approval |
| `docs/plan/7bq4a:prod-backup-verify-stability/PLAN.md` | `docs/specs/prod-backup-verify-stability/SPEC.md` | active | pending delete approval |
| `docs/plan/0012:remote-first-index-sync/PLAN.md` | `docs/specs/remote-first-index-sync/SPEC.md` | active | pending delete approval |
| `docs/plan/0002:small-object-packing/PLAN.md` | `docs/specs/small-object-packing/SPEC.md` | active | pending delete approval |
| `docs/plan/mycnc:status-down-rate/PLAN.md` | `docs/specs/status-down-rate/SPEC.md` | active | pending delete approval |
| `docs/plan/0010:status-popover-dashboard/PLAN.md` | `docs/specs/status-popover-dashboard/SPEC.md` | active | pending delete approval |
| `docs/plan/0008:status-popup-file-logging/PLAN.md` | `docs/specs/status-popup-file-logging/SPEC.md` | active | pending delete approval |
| `docs/plan/0003:sync-logging-durability/PLAN.md` | `docs/specs/sync-logging-durability/SPEC.md` | active | pending delete approval |
| `docs/plan/kaa5e:targets-runs-main-window/PLAN.md` | `docs/specs/targets-runs-main-window/SPEC.md` | active | pending delete approval |
| `docs/plan/0001:telegram-backup-mvp/PLAN.md` | `docs/specs/telegram-backup-mvp/SPEC.md` | active | pending delete approval |
| `docs/plan/0004:telegram-mtproto-storage/PLAN.md` | `docs/specs/telegram-mtproto-storage/SPEC.md` | active | pending delete approval |

## Retained Design Resources

The following design directories intentionally remain at their legacy paths:

- `docs/plan/0005:multi-backup-directories-keyed-restore/design/`
- `docs/plan/0009:endpoints-settings-page/design/`
- `docs/plan/0010:status-popover-dashboard/design/`
- `docs/plan/kaa5e:targets-runs-main-window/design/`

## Compatibility Notes

- Contracts are copied into their canonical topic directories; retained legacy copies are not canonical and remain only until deletion is approved.
- References from legacy Plan files are updated to canonical Spec/contract paths where applicable.
- No runtime code, tests, build configuration, or design asset content is changed by this documentation migration.
