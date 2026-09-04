# TelevyBackup 双色应用图标可行性

## 结论

可以。应以已选定的[第十一组 2 号磁盘与抽象折翼方案](../../assets/brand/televybackup-logo-monochrome-design.png)为唯一图形基础，只替换色彩角色，不增加箭头、快照、云或 Telegram 官方图形。

推荐采用两种着色，白色或透明留白不算第三种品牌色：

| 角色 | 色值 | 应用到已选图形 | 产品含义 |
| --- | --- | --- | --- |
| 存储石墨 | `#263238` | 磁盘机身、外轮廓、指示灯边界 | 本地介质、可靠留存 |
| 传输蔚蓝 | `#1677FF` | 从磁盘上方切出的抽象折翼及其内部折面 | 正在把数据送往远端 |

两色相邻时的 WCAG 对比度约为 `3.21:1`，在白底上的传输蔚蓝对比度为 `4.10:1`。所选方案用中性石墨确保磁盘首先被读作硬件，用蔚蓝作为唯一高饱和传输信号；它不复用错误、离线或排队状态色。现有界面将蓝色用于运行中的状态和进度，故该分工仍与产品视觉语言一致。[`TargetPresentation.swift`](../../macos/TelevyBackupApp/TargetPresentation.swift) 中可见活动状态为 `.blue`。

## 推荐落地边界

- 保持磁盘在视觉重量上大于折翼；折翼只代表“传出”，不能压过“备份源盘”。
- 使用无色背景或白色留白时，图形本身仍只有石墨与传输蔚蓝；不要再加入渐变、高光、投影或第三种装饰色。
- macOS 应用图标画布应保持正方形且不预先裁成圆角，交由系统掩膜；主图形保持居中并预留边距。
- 为深色环境准备同一几何结构的透明画布变体：磁盘使用 `#74869C`，折翼使用 `#5AA9FF`。预览于深色表面时，两层对背景的对比度分别为 `4.80:1` 与 `7.29:1`，不把深色背景烘焙进 SVG。
- 菜单栏状态图标继续是现有的单色 template image；本建议仅针对应用图标，不能改变状态、失败标记或可访问性语义。

## Telegram 边界

本图可使用“抽象折翼”表达远端传输，但不能使用 Telegram 官方纸飞机 Logo、圆形底板、官方素材，或把本方案做成足以让人误认官方 Telegram 应用的构图。Telegram 的 API 条款要求第三方应用告知其使用 Telegram API，同时明确禁止把官方 Telegram Logo 用作应用 Logo；其新闻页虽向文章插图、图表及“forward to Telegram”按钮提供 Logo，也要求避免让人以为在代表 Telegram 官方。故推荐色名为“传输蓝”，而非“Telegram 蓝”，也不应复刻官方标记。

## 风险与验收

- 细线风险：原图的磁盘上沿和折翼边界在小尺寸下可能消失。应在 `16 px`、`32 px`、`64 px` 和 `1024 px` 导出中检查，必要时加粗边界而非添加细节。
- 深色风险：中性石墨在深色背景上可能失去外轮廓。必须分别检查浅色、深色和 Increased Contrast；若需要变体，核心形状必须保持一致。
- 识别风险：若折翼变成白色纸飞机置于蓝色圆形，会接近 Telegram 官方标记，不能采用。
- 色彩语义风险：传输蓝只表示品牌中的“远端传输”，不用于失败、警告或排队；这些状态继续沿用界面的红、橙、灰语义。

验收标准：在任一尺寸下先读出“磁盘”，再读出向上/向外的传输折翼；不需要文字或颜色名称才能理解；两种着色的角色在默认与深色变体中保持不变。

## 依据

- [Apple Human Interface Guidelines: App icons](https://developer.apple.com/design/human-interface-guidelines/app-icons)：建议以少量形状表达一个核心概念、避免纤细线条与尖锐细节、使用方形未掩膜图层并由系统处理圆角；同时要求跨外观保持核心特征一致。
- [Apple Human Interface Guidelines: Color](https://developer.apple.com/design/human-interface-guidelines/color)：颜色应保持语义一致，并要求自定义色在浅色、深色和增强对比度环境中可辨识。
- [Apple Human Interface Guidelines: Dark Mode](https://developer.apple.com/design/human-interface-guidelines/dark-mode)：全彩图片和图标必须在两种外观中可用；仅在单一外观可用时应修改或提供对应变体。
- [Apple Human Interface Guidelines: Accessibility](https://developer.apple.com/design/human-interface-guidelines/accessibility/)：要求评估图标与背景的对比度；非文字状态或图形也需要足够区分度。
- [Telegram API Terms of Service](https://core.telegram.org/api/terms)：第三方 API 应用必须说明使用 Telegram API，且不得把官方 Telegram Logo 用作其应用 Logo。
- [Telegram Press Info](https://telegram.org/press)：官方 Logo 的公开用途限定在新闻插图、图表、转发按钮等场景，并要求不造成官方代表关系的误解。

本记录冻结双色几何与配色角色。已据此生成白底应用图标、透明浅色/深色 UI 变体和透明单色 template 变体；运行时消费边界记录在 macOS App 的 Popover、菜单栏和发布 Specs 中。
