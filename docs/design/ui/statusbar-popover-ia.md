# Status Bar Popover IA (macOS)

## Form factor

- 形态：点击状态栏图标后出现的 popover（悬浮、不可太大）
- 建议尺寸：`360 × 460`（可根据内容在 `400–520` 高度内微调）
- 导航：popover 顶部用 segmented control 在 `Overview / Logs / Settings` 三个视图间切换

## Overview（主视图）

- Header（固定）
  - App 标识 + 标题 `TelevyBackup`
  - 状态 LED（例如：Online/Offline/Syncing）
  - 右侧：`Settings`/`…`（可选，若不放在 segmented 里）
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

## Settings（轻量配置）

- Telegram
  - Bot Token（从 Keychain 读写；popover 里只展示 “已保存/未保存”，避免回显敏感内容）
  - Chat ID / Target（可粘贴）
  - “Test connection” 按钮 + 结果状态
- Schedule
  - 开关：启用定时
  - 频率：Hourly / Daily（下拉）
- Advanced（可折叠）
  - Chunk size（如 `1–10MB`）
  - Concurrency / Rate limit（如有）

## Visual style（Liquid Glass / system material）

- 目标：接近系统 popover 的 **半透明材质 + SF 字体 + 系统分隔线**，避免网页风格块状大卡片
- 重点：行高、间距、阴影与圆角保持克制；交互控件尽量贴近系统控件语义（segmented/toggle/list row）
- 背景材质：以 AppKit `NSVisualEffectView` 或 SwiftUI `Material` 为准（玻璃感来自系统材质）
