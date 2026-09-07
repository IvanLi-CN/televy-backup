# APFS Snapshot Consistency Backup

## 目标

为用户选择的 APFS 卷提供文件系统时间点一致的备份读取视图。开启后，无法创建或挂载快照时备份必须失败，不能退回活动文件系统扫描。

## 范围

- 用户配置按 APFS Volume UUID 保存是否启用快照一致性。
- 用户级 `televybackupd` 继续负责调度、Keychain、扫描、加密、索引和上传。
- root `televybackup-snapshot-helper` 只负责快照探测、创建、挂载、卸载、删除和状态恢复。
- 快照创建使用 `tmutil localsnapshot`，挂载使用 `mount_apfs -s`，删除使用 `diskutil apfs deleteSnapshot -uuid`。
- 每卷最多一个活动 read lease 和一个合并后的待执行任务。
- 符号链接保持为链接；遍历发现嵌套挂载卷时失败，不静默跳过或拆分 source。

## 不在范围内

- 不调用需要额外 entitlement 的 `fs_snapshot_*` API。
- 不把完整备份 daemon 改为 root，不让 helper 读取或上传用户文件。
- 不修改 Time Machine 配置，不支持非 APFS 卷或跨卷 source 自动拆分。
- APFS 快照不提供数据库事务一致性；应用自身 checkpoint 仍由用户负责。

## 配置与状态

- `snapshot_volumes.<volume_uuid>.enabled` 是用户级持久化设置，未知或替换的 UUID 默认关闭。
- 设置页支持逐卷验证和批量启用；不支持的卷保持关闭并展示原因。
- 失败的快照前置检查记录失败 `Backup Run`，但不发布 `Backup Snapshot`。
- 远端备份已成功而快照清理失败时，run 保持成功并进入 `snapshot_cleanup_pending`，阻止同卷下一次快照备份。

## 相关实现合同

- 逻辑 source path 用于历史、基线、索引和远端 manifest。
- 物理 read root 只在 lease 生命周期内使用，扫描和 quick stats 必须从快照挂载点读取。
- source-read-complete 事件表示最后一个源字节已读入加密上传队列；事件后立即释放 lease，上传和索引可继续。
- root helper 以 peer UID、Volume UUID 和 root-owned journal 校验请求；清理始终按 snapshot UUID，不按日期批量删除。

## Related ADRs

- `docs/adr/0007-apfs-snapshot-privileged-helper.md`

