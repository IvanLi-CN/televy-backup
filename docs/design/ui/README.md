# UI Designs

本目录存放 TelevyBackup 的界面设计图（面向 **macOS 状态栏应用** 的 popover 形态，非网页）。

规则：

- `.svg` 作为可编辑源文件（source of truth）。
- `.png` 作为评审/预览基准图（从对应 `.svg` 导出）。
- 若修改了 `.svg`，必须同步更新对应的 `.png`（文件名保持一一对应）。

## Liquid Glass（系统材质 / popover，推荐）

- `docs/design/ui/liquid-glass-popover-overview.png`：主弹窗（Overview）
- `docs/design/ui/liquid-glass-popover-settings.png`：主弹窗（旧：Settings tab；计划 #0005 中改为独立 Settings window）
- `docs/design/ui/liquid-glass-popover-overview.svg`：可编辑源文件（SVG）
- `docs/design/ui/liquid-glass-popover-settings.svg`：可编辑源文件（SVG）
- `docs/design/ui/statusbar-popover-ia.md`：信息架构与交互说明

## Settings window（独立窗口）+ Popover minimal

- `docs/design/ui/settings-window/popover-minimal.png`（及同名 `.svg`）
- `docs/design/ui/settings-window/settings-window-targets.png`（及同名 `.svg`）
- `docs/design/ui/settings-window/settings-window-schedule.png`（及同名 `.svg`）
- `docs/design/ui/settings-window/settings-window-security.png`（及同名 `.svg`）
- `docs/design/ui/settings-window/_preview-popover-minimal.html`
- `docs/design/ui/settings-window/_preview-settings-window.html`

## Status popover dashboard（全局 network + 多目标 + Developer）

- `docs/design/ui/statusbar-popover-dashboard/popover-overview.png`（及同名 `.svg`）
- `docs/design/ui/statusbar-popover-dashboard/popover-overview-empty.png`（及同名 `.svg`，targets=0）
- `docs/design/ui/statusbar-popover-dashboard/developer-window.png`（及同名 `.svg`）
- `docs/design/ui/statusbar-popover-dashboard/_preview-popover.html`

本地预览（macOS）：

```bash
open docs/design/ui/liquid-glass-popover-overview.png
open docs/design/ui/liquid-glass-popover-settings.png
open docs/design/ui/settings-window/popover-minimal.png
open docs/design/ui/statusbar-popover-dashboard/popover-overview.png
```
