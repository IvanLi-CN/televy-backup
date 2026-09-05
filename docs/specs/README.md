# 规格（Spec）总览

本目录是项目唯一的 canonical topic Spec catalog。每个主题目录包含 `SPEC.md`、`IMPLEMENTATION.md` 和 `HISTORY.md`；实现状态与演进记录不再塞入 `SPEC.md`。

当前 catalog 包含 50 个 canonical topics；其中 28 个由 legacy Plan 新建，`0003` 归并到既有的 `sync-logging-durability` 主题。

> Legacy Plan sources remain under `docs/plan/**` until separately approved for deletion. Design assets remain at their existing paths.

## Index

| Topic | Lifecycle | Implementation | Spec | Successor | Notes |
| --- | --- | --- | --- | --- | --- |
| 开发期绕过 Keychain（codesign + vault key） | active | implemented | avoid-keychain-in-dev/SPEC.md | - | legacy plan nvr79; pending delete approval=docs/plan/nvr79:avoid-keychain-in-dev/PLAN.md |
| 后台备份吞吐自适应优化（稳定优先，尽量跑满带宽） | active | implemented | background-backup-adaptive-throughput/SPEC.md | - | legacy plan 7r6p4; pending delete approval=docs/plan/7r6p4:background-backup-adaptive-throughput/PLAN.md |
| Pack 上传后台并发（scan 与 upload 解耦） | active | implemented | background-pack-uploads/SPEC.md | - | legacy plan 0007; pending delete approval=docs/plan/0007:background-pack-uploads/PLAN.md |
| 备份请求队列与前置阶段可观测性 | active | in-progress | backup-request-queue-observability/SPEC.md | - | canonical slug-only topic |
| Backup Snapshot Inspection | active | implemented | backup-snapshot-inspection/SPEC.md | - | canonical slug-only topic |
| Chunking 分块上限调整（按存储模式 + 内存预算） | active | implemented | chunking-max-bytes/SPEC.md | - | legacy plan 0006; pending delete approval=docs/plan/0006:chunking-max-bytes/PLAN.md |
| Settings：配置整包导出/导入（keyed config bundle） | active | implemented | config-bundle-export-import/SPEC.md | - | legacy plan fn4ny; pending delete approval=docs/plan/fn4ny:config-bundle-export-import/PLAN.md |
| Daemon 生命周期与可控退出 | active | in-progress | daemon-lifecycle/SPEC.md | - | legacy spec id k7d2v normalized to slug-only |
| daemon 状态 IPC：替换 file-based 状态源 | active | implemented | daemon-status-ipc/SPEC.md | - | legacy plan 0011; pending delete approval=docs/plan/0011:daemon-status-ipc/PLAN.md |
| Endpoint 去重索引增量化：Remote Delta + 本地物化库 + 周期性 Compaction | active | implemented | endpoint-dedupe-delta-index/SPEC.md | - | legacy spec id 3z7rj normalized to slug-only |
| 索引按 Endpoint 隔离 + 禁止 chat 复用 | active | planned | endpoint-scoped-index-chat-uniqueness/SPEC.md | - | legacy plan r6ceq; pending delete approval=docs/plan/r6ceq:endpoint-scoped-index-chat-uniqueness/PLAN.md |
| 端点索引二级拆分：Endpoint DB（一级）+ Snapshot Filemap DB（二级）+ 严格远端门禁（Fail Fast） | active | in-progress | endpoint-two-level-index/SPEC.md | - | legacy spec id t764g normalized to slug-only |
| Settings：Endpoints 独立配置页（Targets 仅绑定） | active | implemented | endpoints-settings-page/SPEC.md | - | legacy plan 0009; pending delete approval=docs/plan/0009:endpoints-settings-page/PLAN.md |
| CLI events 实时状态与 GUI 进度一致性修复（flush + progress） | active | implemented | events-live-task-ui/SPEC.md | - | legacy plan fwwqp; pending delete approval=docs/plan/fwwqp:events-live-task-ui/PLAN.md |
| 修复 daemon IPC 可靠性（解锁 Recovery Key/Verify） | active | implemented | fix-daemon-ipc-sockets/SPEC.md | - | legacy plan kpmqp; pending delete approval=docs/plan/kpmqp:fix-daemon-ipc-sockets/PLAN.md |
| 修复上传速度显示不正确（MTProto progress + status stream） | active | implemented | fix-upload-rate-display/SPEC.md | - | legacy plan fh5ac; pending delete approval=docs/plan/fh5ac:fix-upload-rate-display/PLAN.md |
| Import bundle: Rebind compare local vs remote latest | active | implemented | import-bundle-rebind-remote-compare/SPEC.md | - | legacy plan w2k9p; pending delete approval=docs/plan/w2k9p:import-bundle-rebind-remote-compare/PLAN.md |
| Import bundle: Targets add "Change folder" button | active | implemented | import-bundle-target-change-directory/SPEC.md | - | legacy plan dxddw; pending delete approval=docs/plan/dxddw:import-bundle-target-change-directory/PLAN.md |
| 索引分级：Remote Index 仅保留每个 Source 最新文件映射 | active | in-progress | index-tiered-filemaps/SPEC.md | - | legacy spec id dyu56 normalized to slug-only |
| macOS：移除 Developer window，把 Diagnostics 整合进主界面 | active | implemented | integrate-developer-diagnostics/SPEC.md | - | legacy plan 7f9wg; pending delete approval=docs/plan/7f9wg:integrate-developer-diagnostics/PLAN.md |
| macOS：dev app variant（Bundle ID / 名称隔离 + menubar DEV 徽标） | active | implemented | macos-dev-app-variant/SPEC.md | - | legacy plan 3ejpg; pending delete approval=docs/plan/3ejpg:macos-dev-app-variant/PLAN.md |
| macOS 明暗主题支持与系统自动切换 | active | implemented | macos-light-dark-theme/SPEC.md | - | legacy spec id fdwoo normalized to slug-only |
| macOS Release Distribution and Product-Managed Daemon | active | in-progress | macos-release-distribution/SPEC.md | - | canonical slug-only topic |
| macOS UI 状态隔离与空闲 CPU 治理 | active | implemented | macos-ui-state-isolation/SPEC.md | - | canonical slug-only topic |
| Main Window：Targets 菜单补齐 “Backup now” | active | implemented | main-window-target-backup-menu/SPEC.md | - | legacy spec id 3rnws normalized to slug-only |
| Master key 轮换（in-place, resumable） | active | planned | master-key-rotation/SPEC.md | - | legacy plan 4fexy; pending delete approval=docs/plan/4fexy:master-key-rotation/PLAN.md |
| macOS 菜单栏同步状态与传输速率 | active | in-progress | menu-bar-sync-status/SPEC.md | - | canonical slug-only topic |
| MTProto dialogs picker（自动选可用 chat_id） | active | implemented | mtproto-dialogs-picker/SPEC.md | - | legacy plan 0013; pending delete approval=docs/plan/0013:mtproto-dialogs-picker/PLAN.md |
| MTProto 空闲 Helper 退出治理 | active | implemented | mtproto-helper-idle-shutdown/SPEC.md | - | legacy spec id cac6x normalized to slug-only |
| MTProto 备份传输提速：更大分片 + 更正确的 FloodWait 处理 + 可调并发/节流 | active | in-progress | mtproto-transfer-speed/SPEC.md | - | legacy spec id hqjd2 normalized to slug-only |
| MTProto upload resilience（retry + heartbeat） | active | implemented | mtproto-upload-resilience/SPEC.md | - | legacy plan njr29; pending delete approval=docs/plan/njr29:mtproto-upload-resilience/PLAN.md |
| 设置窗口独立化与多备份目录（多 Telegram Endpoint + 金钥恢复） | active | implemented | multi-backup-directories-keyed-restore/SPEC.md | - | legacy plan 0005; pending delete approval=docs/plan/0005:multi-backup-directories-keyed-restore/PLAN.md |
| Popover Targets 高度实时自适应与误滚动修复 | active | implemented | popover-targets-live-height/SPEC.md | - | legacy spec id 2e73n normalized to slug-only |
| PR + Label 发布能力 | active | implemented | pr-label-release/SPEC.md | - | legacy spec id n2kbu normalized to slug-only |
| 修复发行版最近备份/验证失败（Telegram 超时、索引误判、瞬态文件、Vault Key 缓存） | active | implemented | prod-backup-verify-stability/SPEC.md | - | legacy plan 7bq4a; pending delete approval=docs/plan/7bq4a:prod-backup-verify-stability/PLAN.md |
| TelevyBackup VERSION-only Release Chain | active | implemented | product-version-release-chain/SPEC.md | - | canonical slug-only topic |
| Release 失败 Telegram 告警接入 | active | implemented | release-failure-telegram-alerts/SPEC.md | - | legacy spec id zx5n2 normalized to slug-only |
| 备份远端索引权威 + 本地自动同步（remote-first） | active | implemented | remote-first-index-sync/SPEC.md | - | legacy plan 0012; pending delete approval=docs/plan/0012:remote-first-index-sync/PLAN.md |
| Settings Control IPC | active | in-progress | settings-control-ipc/SPEC.md | - | canonical slug-only topic |
| 小对象打包降低 Bot API 调用频率 | active | implemented | small-object-packing/SPEC.md | - | legacy plan 0002; pending delete approval=docs/plan/0002:small-object-packing/PLAN.md |
| 状态面板：下载速率实时显示（1s 窗口）与异常波动修复 | active | implemented | status-down-rate/SPEC.md | - | legacy plan mycnc; pending delete approval=docs/plan/mycnc:status-down-rate/PLAN.md |
| 状态弹窗重做：全局网络 + 多目标面板 + 开发者视图 | active | implemented | status-popover-dashboard/SPEC.md | - | legacy plan 0010; pending delete approval=docs/plan/0010:status-popover-dashboard/PLAN.md |
| 状态弹出界面移除日志页（日志仅落盘） | active | implemented | status-popup-file-logging/SPEC.md | - | legacy plan 0008; pending delete approval=docs/plan/0008:status-popup-file-logging/PLAN.md |
| backup 主流水线并行化（scan+upload）与进度语义修复 | active | in-progress | streaming-backup-pipeline/SPEC.md | - | legacy spec id dmts3 normalized to slug-only |
| Sync Logging Durability and Local Diagnostics | active | implemented | sync-logging-durability/SPEC.md | - | legacy plan 0003; pending delete approval=docs/plan/0003:sync-logging-durability/PLAN.md |
| Targets 主界面与执行记录（按目标聚合 backup/restore/verify） | active | implemented | targets-runs-main-window/SPEC.md | - | legacy plan kaa5e; pending delete approval=docs/plan/kaa5e:targets-runs-main-window/PLAN.md |
| TelevyBackup MVP（Telegram 存储 + 差异备份） | active | implemented | telegram-backup-mvp/SPEC.md | - | legacy plan 0001; pending delete approval=docs/plan/0001:telegram-backup-mvp/PLAN.md |
| Telegram 通信升级为 MTProto API（MTProto-only，移除 Bot API） | active | implemented | telegram-mtproto-storage/SPEC.md | - | legacy plan 0004; pending delete approval=docs/plan/0004:telegram-mtproto-storage/PLAN.md |
| 支持 `.televyignore` 的文件/目录忽略能力 | active | implemented | televyignore-target-ignore/SPEC.md | - | legacy spec id g7gt3 normalized to slug-only |
| 统一进度条规范（含 Prepare 并行）与四处 UI 对齐 | active | implemented | unified-backup-progress-prepare/SPEC.md | - | legacy spec id z324m normalized to slug-only |

## Migration Rules

- New topics use stable lowercase kebab-case directory names without IDs.
- `active` topics may be fully implemented; `superseded` and `archived` are reserved for explicit replacement or retirement.
- Legacy Plan and contract deletion requires a separate owner approval after the complete mapping is reviewed.
