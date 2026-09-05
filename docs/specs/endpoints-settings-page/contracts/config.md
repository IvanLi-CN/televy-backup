# Config contracts（internal）

本文件描述本计划新增的 **UI 偏好设置（UserDefaults）** 键，用于 “Targets 默认 endpoint 选择” 的启发式规则。

## Scope

- 仅用于 macOS app 的 Settings window；
- 不写入 `config.toml`；
- 仅 internal，可随实现演进，但需要保持“旧版本写入的数据不会导致崩溃/异常默认值”。

## Keys

### `ui.settings.endpoints.lastTouchedEndpointId`

- Type: String
- Meaning: 最近一次 “创建/更新 endpoint” 所对应的 endpoint id
- Update triggers（任一满足）：
  - 在 `Endpoints` 页面新增 endpoint 成功后
  - 在 `Endpoints` 页面对某 endpoint 的关键字段保存成功后（chat id / bot token / api hash / clear sessions）
- Notes: 若该 id 在当前配置中已不存在，则忽略并回退到默认策略。

### `ui.settings.targets.lastSelectedEndpointId`

- Type: String
- Meaning: 最近一次在 `Targets` 页面为某 target 选择并保存的 endpoint id
- Update triggers:
  - 在 `Targets` 页面 picker 选择发生变更并保存成功后

### `ui.settings.endpoints.lastTouchedAt` / `ui.settings.targets.lastSelectedAt`

- Type: Double（Unix timestamp seconds）或 Int64（实现自选其一，需保持一致）
- Meaning: 对应 id 的“最近时间点”
- Default: 若缺失则视为 `0`（很久以前）

## Default endpoint selection algorithm（summary）

当 `Targets` 详情需要一个默认 endpoint（例如新增 target 或进入详情且当前绑定为空/无效）时：

1. 若 `ui.settings.endpoints.lastTouchedAt` 与 `ui.settings.targets.lastSelectedAt` 都存在：
   - 取时间更大的那个对应的 endpoint id 作为默认；
2. 否则取存在者；
3. 若选出的 endpoint id 不存在于当前 endpoints 列表：
   - 回退为 endpoints 列表的首项（按 natural sort 后的首项）。

## Sorting

Endpoint picker / list 的展示顺序：natural sort（示例：`ep_2` < `ep_10`）。
