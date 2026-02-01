# Targets 主界面与执行记录：UI 截图（#kaa5e）

本目录存放本计划的主界面（Targets / Runs）截图（来自真实 app 的 UI demo 场景，用脚本自动截取）。

规则：

- `.png` 为截图产物（建议不要手工编辑，避免与真实 UI 偏离）。

## 文件清单

- `main-window-targets.png`：主界面 Targets 列表（包含 `Restore…` / `Verify`）。
- `main-window-target-detail.png`：Target 详情 + 执行记录摘要（按 target 分组）。

## 重新生成截图（可重复执行）

```bash
./scripts/macos/build-app.sh
./scripts/macos/capture-main-window.sh main-window-targets docs/plan/kaa5e:targets-runs-main-window/design/main-window-targets.png
./scripts/macos/capture-main-window.sh main-window-target-detail docs/plan/kaa5e:targets-runs-main-window/design/main-window-target-detail.png
```
