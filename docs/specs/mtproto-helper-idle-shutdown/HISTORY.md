# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## 方案概述（Approach, high-level）

- 把“helper 进程退出”从隐式依赖 pipe EOF，提升为显式内部协议能力。
- core 端所有 teardown 入口共享同一条 helper 终止路径，避免 respawn、drop、异常恢复行为分叉。
- daemon 不新增新的 stop surface，只继续复用既有 cache clear 生命周期，降低产品面变更。
- macOS 历史摘要从“整文件读取”收敛为“前缀 + 后缀窗口”解析，既保留 `run.start` / `run.finish` 所需字段，也避免 UI 因超大日志卡住后误报空历史。


## 变更记录（Change log）

- 2026-03-22：新增 MTProto helper idle teardown 规格，锁定“空闲即退出 helper、无新增用户可见 stop 入口”的实现边界与验收标准。
