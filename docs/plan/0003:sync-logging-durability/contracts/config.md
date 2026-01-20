# Config Contracts（logging）

> Kind: Config（internal）

## 1) 环境变量（Env vars）

### `TELEVYBACKUP_LOG`

用途：配置日志过滤规则（等级与 targets），用于控制写入“每轮同步日志文件”的内容量。

- 类型：string
- 可选：是
- 默认：`debug`
- 格式：兼容 `tracing-subscriber` 的 `EnvFilter` 语法（与 `RUST_LOG` 等价的语法习惯）

优先级（从高到低）：

1. `TELEVYBACKUP_LOG`
2. `RUST_LOG`
3. 默认规则（debug）

示例：

- `TELEVYBACKUP_LOG=debug`
- `TELEVYBACKUP_LOG=info`
- `TELEVYBACKUP_LOG=info,televy_backup_core=debug,televybackup=debug,televybackupd=debug`
- `TELEVYBACKUP_LOG=warn,reqwest=info`

### `TELEVYBACKUP_LOG_DIR`

用途：覆盖日志目录（例如 Homebrew service 期望落到 `$(brew --prefix)/var/log/...`）。

- 类型：path（string）
- 可选：是
- 默认：见 `./file-formats.md`

约束：

- 必须是可写目录；不存在时允许创建（实现阶段决定是否自动创建与失败策略）。

## 2) 安全与隐私（Security/Privacy）

- `TELEVYBACKUP_LOG` 与 `TELEVYBACKUP_LOG_DIR` 的值本身不得包含 secrets（例如 token）。
- 日志内容禁止记录：
  - Telegram bot token、master key、任何密文/明文 payload
  - Keychain 读取得到的敏感值
- 允许记录（最小必要）：
  - snapshot_id、task_id/run_id、source_path（或其 hash/相对路径）、字节/计数统计、错误码与错误信息（去敏）
