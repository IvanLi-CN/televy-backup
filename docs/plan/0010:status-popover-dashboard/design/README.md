# Status popover 设计图（#0010）

本目录存放本计划的 Popover 设计图与交互/显示口径说明。

规则：

- `.svg` 为可编辑源文件（source of truth）。
- `.png` 为预览图（从对应 `.svg` 导出，文件名一一对应）。

## 文件清单

- `popover-overview.svg` / `popover-overview.png`
  - Overview：全局网络 + targets 列表面板。
- `popover-overview-empty.svg` / `popover-overview-empty.png`
  - Overview（empty）：当 `targets=0` 时的引导与跳转按钮（打开 Settings 添加 targets）。
- `developer-window.svg` / `developer-window.png`
  - Developer window：全局状态 + per-target 原始字段 + activity（用于排障）。
- `_preview-popover.html`
  - 本地预览页面（浏览器打开即可）。

## 信息架构与交互说明（冻结口径）

### 1) 导航

- Popover 无 tabs；打开即为 Overview。
- 尺寸：宽度固定 `360`；高度按内容自适应，最大高度 `720`（高宽比 `2:1` 上限）；targets 溢出时区域滚动承载长列表（header/global 固定）。

### 2) Overview：全局 + targets

**全局（NETWORK）**

- `Up/Down (Now)`：实时速率。
- `Up/Down (Session)`：自 UI 启动以来累计值（非持久化；重启清零）。
- `Last updated`：显示 freshness；stale 时显著提示。

**Targets**

每个 target 行包含：

- 标识：`label`（主）；`source_path`（副，截断，空间不足时可省略）。
- 运行状态：`Idle / Running / Failed / Stale`（颜色语义见下）。
- 指标：`↑` 实时速率；体积（优先 `bytesUploaded`，其次 `bytesRead`）；最近一次运行时间/用时；运行中展示进度条与已用时。
- 列表右上角仅展示 targets 总数（例如 `8 targets`），不暗示当前滚动位置。
- 当 `targets=0`：展示 empty state，引导用户打开 Settings 添加 targets，并提供主按钮 `Open Settings…`。

### 3) Dev：详细状态（尽量充分）

- 入口：Settings window 提供“Developer…”入口（增加入口，不新增 Settings 页面），点击打开独立窗口
- Global:
  - `schemaVersion / generatedAt / source`（用于确认数据源）
  - `global.up/down` 与 totals（原始数值）
  - UI 侧 `receivedAt`（若实现侧提供）与 `staleAgeMs`
- Targets（每个 target 一组）：
  - `targetId / label / sourcePath / endpointId / enabled`
  - `state` + `progress` 原始计数（files/chunks/bytes）
  - `lastRun`（finishedAt/duration/status/errorCode）
 - Activity（必须）：
   - 最近 N 条“工作活动”时间线（快照更新、文件落盘、阶段推进、错误码出现、stale 触发等）

## 视觉与状态语义（冻结口径）

- Running：强调色 `blue`；进度条为蓝。
- Succeeded：`green`（仅在“最近一次”摘要中出现）。
- Failed：`red`；显示短 `errorCode`（长错误信息仅在 Dev 展示）。
- Stale：`orange`（提示“数据未更新”而非业务失败）。
- 主要数字使用等宽字体（monospace）以减小跳动。
- Targets 列表为滚动区域：必须裁剪（clip）到父容器内，且使用系统风格滚动条（overlay scrollbar）提示可滚动。
- 滚动边缘渐隐：建议对 targets 列表“内容本身”做 alpha mask（上下渐隐），避免用不透明/有色遮罩覆盖半透明背景导致脏块感。
- 内容边距：targets 列表建议 `contentInsetTop=8–12px`、`contentInsetBottom=16px`，避免首/尾行贴边或被圆角裁切。
- 渐隐显示规则（需实现按滚动位置动态控制）：
  - 仅当 targets 列表可滚动时显示；
  - 未到顶部：显示顶部渐隐；未到底部：显示底部渐隐；到顶/到底则关闭对应边的渐隐。

## 导出 PNG（可重复执行）

```bash
rsvg-convert -o docs/plan/0010:status-popover-dashboard/design/popover-overview.png docs/plan/0010:status-popover-dashboard/design/popover-overview.svg
rsvg-convert -o docs/plan/0010:status-popover-dashboard/design/popover-overview-empty.png docs/plan/0010:status-popover-dashboard/design/popover-overview-empty.svg
rsvg-convert -o docs/plan/0010:status-popover-dashboard/design/developer-window.png docs/plan/0010:status-popover-dashboard/design/developer-window.svg
```
