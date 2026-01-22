# Config Contracts（chunking max_bytes cap）

> Kind: Config（internal）
>
> Format: `config.toml`

## 1) `[chunking]`

- 范围（Scope）: internal
- 变更（Change）: Modify（本计划将修改/明确 `max_bytes` 的“有效上限语义”）

### 字段

- `chunking.min_bytes`: number
  - 必填：是
  - 约束：`> 0`
- `chunking.avg_bytes`: number
  - 必填：是
  - 约束：`> 0`
- `chunking.max_bytes`: number
  - 必填：是
  - 约束：`> 0`

### 关系约束（必须满足）

- `chunking.min_bytes <= chunking.avg_bytes <= chunking.max_bytes`

### 上限语义（Max cap semantics）

chunking 产生的原始 chunk 会被加密并做 framing，形成最终上传的 blob。

framing 开销（当前实现）：`1(version) + 24(nonce) + 16(tag) = 41 bytes`。

因此当 `chunking.max_bytes = N` 时，最坏情况下上传 blob 的大小为 `N + 41`。

#### MTProto（仓库为 MTProto-only）

Telegram 客户端侧能力参考：非 Premium 用户单文件 2GB、Premium 用户单文件 4GB（由 Telegram 官方说明）。

但本项目当前实现（chunk→encrypt→upload）是以内存处理为主，并且 MTProto helper 会把上传 bytes 再读入一份缓冲；
为控制内存峰值、上传超时与失败面，本计划引入一个 **MTProto 上传单文件工程上限**（显著小于 Telegram 理论上限）：

- 约束：`chunking.max_bytes + 41 <= MTProtoEngineeredUploadMaxBytes`
- `MTProtoEngineeredUploadMaxBytes`（建议默认）：`128MiB`（定义为“单次 upload_document 的 bytes 上限”，包含 framing；精确值以实现常量为准）

## 2) 与 pack 的关系（pack interaction）

本项目存在“pack 小对象降低上传次数”的机制。

当单个加密 blob 过大时，会绕过 pack，直接作为单独 document 上传（避免超过 pack hard max）：

- 预期：即便 `chunking.max_bytes > PACK_MAX_BYTES`，系统仍应选择 direct upload 路径而非失败。

（如实现阶段发现现有逻辑无法满足该预期，需要在本计划内补齐回归测试并修复。）

### Pack sizing（本计划拟调整的默认值；internal）

- `PACK_MAX_BYTES`：拟调整为 `128MiB`（与 `MTProtoEngineeredUploadMaxBytes` 对齐）
- `PACK_TARGET_BYTES`：拟调整为 `64MiB`（减少“单 pack entries 数量过大”的风险；并仍可显著减少上传次数）
- `PACK_TARGET_BYTES` jitter：建议引入 `±8MiB` 的抖动区间（每个 pack 的 flush 阈值不同；避免文件尺寸过于规律）
- `PACK_MAX_ENTRIES_PER_PACK`：新增一个 entries 上限（达到上限就强制 flush），用于避免一个 pack 里塞入过多小文件（过多 entries）
  - Owner decision: `32`
- `PACK_ENABLE_MIN_OBJECTS`：保持 `10` 不变（达到该数量后可进入 pack 模式）

## 3) 兼容性与迁移（Compatibility / migration）

- 本计划不改变 `config.toml` 的字段结构，仅改变/明确 `chunking.max_bytes` 在不同存储模式下的有效上限语义与错误提示。
