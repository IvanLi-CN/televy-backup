# File formats Contracts（fallback）

## `status.json`

- Change: Modify（从“主数据源”降级为 fallback）
- 仍要求原子写（temp + rename）以避免读到半写内容

