# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## Change log

- 2026-04-08: 创建 spec，冻结范围、验收口径与 merge-ready 收口目标。
- 2026-04-08: 完成主题桥接、Popover/Main/Settings 适配、Swift 回归、app 构建与 light/dark 视觉证据生成。
- 2026-04-08: 收敛主窗口/设置窗口的 macOS 原生风格细节，改用激活态窗口截图作为最终视觉证据，消除非激活 titlebar/toolbar 的低对比度误导。
- 2026-04-09: 修复 Settings `Targets` / `Endpoints` 左侧 sidebar 在暗色模式下回落成纯黑背景的回归，补充真实运行窗口截图并确认左栏/footer 共享同一 sidebar surface。
- 2026-04-10: 修复 UI demo / snapshot 污染真实配置目录的问题：截图脚本显式隔离 `.dev/ui-snapshot/{config,data}`，app 在 `TELEVYBACKUP_UI_DEMO=1` 且未传目录时也会自动回落到临时 sandbox。
