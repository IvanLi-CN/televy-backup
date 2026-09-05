# History

## Provenance

- Legacy source: `docs/plan/3ejpg:macos-dev-app-variant/PLAN.md`.
- Legacy identifier is preserved in the catalog Notes field for traceability.

## Durable Rationale and Change Record

## 方案概述（Approach）

### 变体开关

- 新增环境变量：`TELEVYBACKUP_APP_VARIANT=dev|prod`
  - `build-app.sh` 默认 `prod`
  - `run-app.sh` 默认 `dev`（可手动覆盖）

### `.app` 目录名与 Bundle ID

- `.app` 目录名：
  - prod：`target/macos-app/TelevyBackup.app`
  - dev：`target/macos-app/TelevyBackup Dev.app`
- `Info.plist`：
  - `CFBundleIdentifier`：按变体写入（prod/dev 不同）
  - `CFBundleDisplayName` / `CFBundleName`：按变体写入（prod/dev 不同）
  - `CFBundleExecutable`：固定为 `TelevyBackup`（避免可执行文件名含空格）

### run 脚本的 quit / 清理

- quit：使用 AppleScript 的 application id（bundle id）退出，避免误关另一变体：
  - `tell application id "com.ivan.televybackup.dev" to quit`（dev）
- `pkill/pgrep`：按当前 `.app` 内的可执行路径匹配，避免误杀另一变体。
  - dev 变体默认禁用 Keychain；如需要 prod-like 行为，可显式传 `--enable-keychain`（脚本会在 `TELEVYBACKUP_DISABLE_KEYCHAIN=0` 时传入）。

### dev 状态栏图标（DEV 徽标）

- 根据 `Bundle.main.bundleIdentifier` 判断是否 dev（以 `.dev` 结尾）。
- dev 模式生成一张合成模板图：
  - 基底：现有 SF Symbol（`externaldrive`）
  - 右下角叠加圆角矩形徽标块，并用 `destinationOut` “挖空”出 `DEV` 文本
  - 最终 `isTemplate = true`，确保深色/浅色菜单栏自适配

## Compatibility

- Legacy source retained pending delete approval: `docs/plan/3ejpg:macos-dev-app-variant/PLAN.md`.
