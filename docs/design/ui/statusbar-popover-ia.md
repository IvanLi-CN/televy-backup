# Status Bar Popover IA (macOS)

## Form factor

- 形态：点击状态栏图标后出现的 popover（悬浮、不可太大）
- 建议尺寸：`360 × 460`（可根据内容在 `400–520` 高度内微调）
- 导航：popover 顶部用 segmented control 在 `Overview / Logs` 两个视图间切换；`Settings` 通过右上角齿轮按钮打开独立 Settings window
- 快捷键：支持 `⌘,` 打开 Settings window（符合 macOS 习惯）

## Overview（主视图）

- Header（固定）
  - App 标识 + 标题 `TelevyBackup`
  - 状态 LED（例如：Online/Offline/Syncing）
  - 右侧：齿轮（打开 Settings window）/`…`（可选）
- 状态卡片
  - 当前任务状态：Idle / Uploading / Verifying / Error
  - 进度条（仅在运行时显示）
  - 关键统计：uploaded / dedupe / duration（数字建议用等宽字）
- Quick actions
  - Primary：`Run backup now`
  - Secondary：`Open logs`
- Footer
  - Telegram storage 连接状态 + 简短提示（错误时提供一行可操作引导）

## Logs（列表）

- 以 `NSTableView`/macOS 列表行的语义呈现：时间 + 结果 + 可展开详情
- 支持复制错误/导出日志（popover 内做“复制”优先，导出可跳到独立窗口）

## Settings window（独立窗口）

- Settings 不再作为 popover 的 tab；popover 通过右上角齿轮按钮打开独立 Settings window。
- Settings window 承载：backup targets（多目录）、Telegram endpoint 绑定、schedule、安全（recovery key）等配置。

## Visual style（Liquid Glass / system material）

- 目标：接近系统 popover 的 **半透明材质 + SF 字体 + 系统分隔线**，避免网页风格块状大卡片
- 重点：行高、间距、阴影与圆角保持克制；交互控件尽量贴近系统控件语义（segmented/toggle/list row）
- 背景材质：以 AppKit `NSVisualEffectView` 或 SwiftUI `Material` 为准（玻璃感来自系统材质）
