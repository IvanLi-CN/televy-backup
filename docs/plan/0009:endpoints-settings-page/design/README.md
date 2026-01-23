# Settings window 设计图（#0009）

本目录存放本计划的 Settings window 设计图与说明。

规则：

- `.svg` 为可编辑源文件（source of truth）。
- `.png` 为预览图（从对应 `.svg` 导出，文件名一一对应）。

## 文件清单

- `settings-window-targets.svg` / `settings-window-targets.png`
  - `Targets` 页：Endpoint 区域改为“绑定 + 只读摘要”，并提供 `Edit…` 跳转到 `Endpoints`。
- `settings-window-endpoints.svg` / `settings-window-endpoints.png`
  - `Endpoints` 页：集中管理 endpoints（列表 + 详情编辑 + test connection 等动作）。
- `_preview-settings-window.html`
  - 本地预览页面（浏览器打开即可）。

## 交互说明（冻结口径）

### 1) Targets 页：Endpoint 绑定 + 只读摘要

- Endpoint picker：仍在 Targets 页完成 endpoint 绑定。
- 下方摘要：仅展示信息（不可编辑、不提供 secrets/session 的写入按钮）。
- `Edit…` 按钮：跳转到 Endpoints 页，并自动选中当前 endpoint（高亮 + 详情对应）。

### 2) Endpoints 页：集中编辑与管理

- 左侧列表：展示 endpoints；底部 `+ / –` 用于新增与删除。
- 右侧详情：承载 endpoint 的可编辑字段与动作（保存 bot token、保存 api hash、清除 session、test connection）。
- 删除策略：若 endpoint 被任一 target 引用，删除操作会被阻止，并提供“一键跳转 Targets”的引导；跳转后默认选中一个引用该 endpoint 的 target 方便用户改绑/解除引用。

### 3) 默认 Endpoint 选择与排序

- 默认 endpoint：取“最近一次创建/更新 endpoint”与“最近一次被选为 target 的 endpoint”两者中时间更新的那个。
- Picker/list 顺序：natural sort（例如 `ep_2` 排在 `ep_10` 之前）。

## 导出 PNG（可重复执行）

```bash
rsvg-convert -o docs/plan/0009:endpoints-settings-page/design/settings-window-targets.png docs/plan/0009:endpoints-settings-page/design/settings-window-targets.svg
rsvg-convert -o docs/plan/0009:endpoints-settings-page/design/settings-window-endpoints.png docs/plan/0009:endpoints-settings-page/design/settings-window-endpoints.svg
```
