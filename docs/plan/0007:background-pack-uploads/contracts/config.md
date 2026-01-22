# 配置（Config）

## `telegram.rate_limit`（上传/下载的速率限制）

- 范围（Scope）: external
- 变更（Change）: Modify
- 负责人（Owner）: app/core
- 使用方（Consumers）: CLI / daemon / macOS app

### 适用范围（Applies to）

- 本配置项的语义来源于计划 #0004：作为 Telegram 侧上传/下载的统一并发与节流配置。
- 本计划 #0007 的交付范围仅包含 **backup 的上传侧**（调用 `Storage::upload_document`），覆盖所有上传类型：
  - pack 上传
  - “直传 blob”（无法/不应打包时的单对象上传）
  - index 上传（index parts + manifest 等）
- restore/verify 的下载侧是否受同一配置约束，不在本计划范围内（保持现状；如需调整另立计划）。

### 字段（Fields）

| 字段 | 类型 | 必填 | 默认值 | 说明 |
| --- | --- | --- | --- | --- |
| `max_concurrent_uploads` | int（u32） | 是 | `2` | 同时进行的上传任务最大并发数（worker 数上限） |
| `min_delay_ms` | int（u32） | 是 | `250` | 上传启动的最小间隔（毫秒）；用于避免触发限速/抖动放大 |

### 行为语义（Semantics）

- 并发：任一时刻“正在执行 upload”的任务数不超过 `max_concurrent_uploads`。
- 最小间隔：**两次 upload 启动**之间的时间间隔不小于 `min_delay_ms`（全局节流；多 worker 不应叠加速率）。

### 校验（Validation）

- `max_concurrent_uploads >= 1`
- `min_delay_ms >= 0`

### 兼容性（Compatibility）

- 兼容旧配置文件：字段已存在但此前可能不生效；本计划会让其语义生效，属于“行为增强”。
