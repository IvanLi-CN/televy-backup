# History

## Provenance

- Legacy source: `docs/plan/0009:endpoints-settings-page/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/plan/0009:endpoints-settings-page/PLAN.md`: 冻结范围、验收、开放问题
- `docs/plan/0009:endpoints-settings-page/design/README.md`: 设计图说明（交互/跳转规则）


## 方案概述（Approach, high-level）

- 将 endpointEditor 从 `Targets` 详情中解耦：
  - `Targets` 仅保留 picker + 只读摘要；
  - 新增 `Endpoints` 页面承载编辑与动作（save secrets / clear sessions / test connection）。
- 从 `Targets` 跳转到 `Endpoints` 采用“带参数的选择”：
  - `Edit endpoint…` 触发：切换 section=Endpoints，并把 selectedEndpointId 设为当前 target.endpoint_id。
- 删除保护以“引用关系”为准：`targets[].endpoint_id` 指向的 endpoint 禁止删除。


## 变更记录（Change log）

- 2026-01-23: 创建计划并补齐初版范围/验收与设计产物链接。
- 2026-01-24: 已实现 Endpoints 独立页、Targets 仅绑定 + 只读摘要、删除保护与默认选择启发式（UserDefaults）。

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/0009:endpoints-settings-page/PLAN.md`.
