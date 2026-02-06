# Import bundle: Targets add "Change folder" button（#dxddw）

## 状态

- Status: 已完成
- Created: 2026-02-06
- Last: 2026-02-06

## 背景 / 问题

- 在 Settings → Backup Config → Import backup config（inspect 结果页）里，Targets 列表目前只有路径展示与冲突处理（needs_resolution）时才提供 rebind。
- 现实场景里，即使预检为 OK，用户也可能希望把导入的 target 指向本机的另一个目录（例如磁盘挂载点不同/路径结构不同/想改新目录）。

## 目标

- 在 import bundle 的 Targets 条目里增加一个“更换目录（Choose/Change folder）”按钮。
- 用户可为任意选中的 target 选择一个新的本地目录（rebind），并在 apply 时生效。

## 范围（In/Out）

### In scope

- macOS UI：Import backup config 结果页 Targets 列表
  - 每个 target 显示一个 Change/Choose 按钮（用于选择目录）。
  - 若用户为某个 target 选择了新目录，UI 显示“实际将应用的路径”。
- Apply payload：对非冲突 target 也允许提交 `resolutions[targetId] = { mode: "rebind", newSourcePath }`。

### Out of scope

- 不变更 CLI 的 conflict 检测规则。
- 不新增自动迁移/复制用户目录内容（仅更新配置中的 source_path）。

## 验收标准（Acceptance Criteria）

- Given 在 import bundle inspect 结果页，When 查看任意 target 条目，Then 可见“Change…/Choose…”按钮。
- Given 点击按钮并选择一个文件夹，Then 条目显示的新路径会更新，并在 Apply 后写入本机 settings。
- Given 用户未选择新目录，Then 行为与现在一致（不发送多余 resolution，不改变导入路径）。
- Given 用户将某个 target 设为 rebind 但未选目录（例如取消选择面板），Then Apply 按钮不可用。

## 测试 / 验证

- 构建 macOS app（确保 Swift 编译通过）。
- 使用内置 UI snapshot 机制产出一张 import result 页截图（包含新按钮）。
