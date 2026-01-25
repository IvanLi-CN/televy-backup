# Status Bar Popover IA (macOS)

## Form factor

- 形态：点击状态栏图标后出现的 popover（悬浮、不可太大）
- 尺寸：宽度固定 `360`；高度按内容自适应，最大高度 `720`（高宽比 `2:1` 上限；targets 列表溢出时滚动承载）
- 导航：popover 本身不提供 tabs；`Settings` 通过右上角齿轮按钮打开独立 Settings window（Dev 入口也在 Settings）
- 快捷键：支持 `⌘,` 打开 Settings window（符合 macOS 习惯）

## Overview（主视图）

- Header（固定）
  - App 标识 + 标题 `TelevyBackup`
  - 状态 LED（例如：Online/Offline/Syncing）
  - 右侧：齿轮（打开 Settings window）/`…`（可选）
- 全局状态（NETWORK）
  - 实时上下行速率（↑/↓）
  - 自 UI 启动后累计上下行流量（session totals）
  - freshness：Last updated / stale 提示
- Targets 列表（每个 backup target 一行）
  - 实时上行（业务口径 bytesUploaded）、体积指标（bytesRead/bytesUploaded）、运行状态
  - 最近一次运行时间/用时（失败时显示短 error code）
  - 运行中显示进度条 + 已用时（可选 ETA）
  - 列表可滚动时：上下边缘渐隐提示仅在“未到顶/未到底”时显示（使用内容 alpha mask，避免遮罩染色）
- Empty（targets=0）
  - 显示引导文案 + 主按钮 `Open Settings…`（跳转到 Settings 添加 targets）
- Footer
  - Telegram storage 连接状态 + 简短提示（错误时提供一行可操作引导）

## Dev（开发者视图）

- 入口：Settings window 提供 `Developer…` 入口（增加入口，不新增 Settings 页面），点击打开独立窗口；Popover 不承载 Dev 页面
- Global：schemaVersion/source/generatedAt + 原始 up/down/totals + staleAge
- Per-target：targetId/sourcePath/endpointId/enabled + state + progress 原始计数 + lastRun/errorCode
 - Activity：可见的工作活动时间线（快照更新、写入、进度推进、错误/告警等）

## Settings window（独立窗口）

- Settings 不再作为 popover 的 tab；popover 通过右上角齿轮按钮打开独立 Settings window。
- Settings window 承载：backup targets（多目录）、Telegram endpoint 绑定、schedule、安全（recovery key）等配置。

## Visual style（Liquid Glass / system material）

- 目标：接近系统 popover 的 **半透明材质 + SF 字体 + 系统分隔线**，避免网页风格块状大卡片
- 重点：行高、间距、阴影与圆角保持克制；交互控件尽量贴近系统控件语义（segmented/toggle/list row）
- 背景材质：以 AppKit `NSVisualEffectView` 或 SwiftUI `Material` 为准（玻璃感来自系统材质）
