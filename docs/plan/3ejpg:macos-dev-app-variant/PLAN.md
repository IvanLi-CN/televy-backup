# macOS：dev app variant（Bundle ID / 名称隔离 + menubar DEV 徽标）（#3ejpg）

## 状态

- Status: 已完成
- Created: 2026-02-08
- Last: 2026-02-09

## 背景 / 问题

- 目前开发构建出来的 `.app` 与正式版使用相同的 `CFBundleIdentifier` / App 名称，导致在同一台电脑上安装/运行正式版时，开发体验不友好（互相“像是同一个 app”，难以并存与区分）。
- 状态栏（menubar）图标目前也与正式版一致，开发/正式更容易混淆。

## 目标 / 非目标

### Goals

- 提供 prod/dev 两个 macOS app 变体（只改 Bundle ID + 显示名，不改变核心功能逻辑）：
  - prod：`com.ivan.televybackup`，显示名 `TelevyBackup`
  - dev：`com.ivan.televybackup.dev`，显示名 `TelevyBackup Dev`
- dev 默认禁用 Keychain（避免开发过程访问/污染正式版 Keychain；仍可通过参数显式启用用于 prod-like 测试）。
- dev 状态栏图标带清晰的 `DEV` 字样（图标内叠加徽标），避免误操作。
- 两个 `.app` 可同时存在并可同时运行（互不覆盖、互不抢占）。
- `scripts/macos/run-app.sh` 默认启动 dev 变体；打包/发布链路保持 prod 不变。

### Non-goals

- 不做 notarization / 发行签名链路。
- 不修改 Rust crates 的包名或二进制名（`televybackup*` 维持现状）。
- 不变更 Homebrew/cask 模板（仍指向 prod）。

## 范围（In/Out）

### In scope

- `scripts/macos/build-app.sh`：支持 `TELEVYBACKUP_APP_VARIANT=dev|prod`，生成对应 `.app` 与 `Info.plist`。
- `scripts/macos/run-app.sh`：默认 `dev`；退出/清理逻辑仅影响当前变体。
- `macos/TelevyBackupApp/TelevyBackupApp.swift`：dev menubar 图标叠加 `DEV` 徽标。

### Out of scope

- 其他功能与配置语义不变（仍由 `--data-dir/--config-dir` 等参数控制）。

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

## 验收标准（Acceptance Criteria）

- Given 同一台电脑上已有正式版（prod），When 启动本地开发版（dev），Then 两者可同时运行且不会互相替换/覆盖。
- Given dev 版正在运行，When 观察状态栏图标，Then 图标内可清晰看到 `DEV` 字样，并与 prod 区分明显。
- Given 同时运行 dev + prod，When 执行 `scripts/macos/run-app.sh` 的重启/清理逻辑，Then 只影响 dev，不误杀 prod。
- Given 启动 dev 变体（无额外参数），When app 启动并拉起 daemon/CLI，Then 默认 `TELEVYBACKUP_DISABLE_KEYCHAIN=1`（禁用 Keychain）；如显式传 `--enable-keychain` 则允许启用 Keychain 用于测试。
- Given 未设置 `TELEVYBACKUP_APP_VARIANT` 且运行 `build-app.sh`，Then 默认产物与当前仓库行为一致（`TelevyBackup.app`，bundle id 为 prod）。

## 测试 / 验证

- Dev：运行 `./scripts/macos/run-app.sh`，确认 `.app` 名称、bundle id、menubar 图标。
- Prod（手工）：设置 `TELEVYBACKUP_APP_VARIANT=prod` 后运行 `./scripts/macos/build-app.sh` 并启动，确认仍为 `TelevyBackup.app` 与 prod bundle id。

## 里程碑（Milestones）

- [x] M1: `build-app.sh` / `run-app.sh` 支持 dev/prod 变体（bundle id/name 分离）
- [x] M2: dev menubar 图标叠加 `DEV` 徽标
- [x] M3: 主人验收：dev+prod 可并存运行，且 `DEV` 徽标清晰可辨
