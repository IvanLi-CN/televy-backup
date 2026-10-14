# File formats Contracts（fallback）

## `status.json`

- Change: Modify（从“主数据源”降级为 fallback）
- 仍要求原子写（temp + rename）以避免读到半写内容

## Optional run failure diagnosis

`targets[].lastRun.errorReason` is an optional camelCase string, omitted when unavailable. It contains only a bounded category (`protocol_mismatch`, `permission_denied`, `service_unavailable`, `mount_helper_unavailable`, `insufficient_space`, `cleanup_pending`, or `request_timed_out`). It supplements `errorCode` without changing existing error semantics or schema version. Older consumers ignore it; current consumers accept older snapshots with no reason.
