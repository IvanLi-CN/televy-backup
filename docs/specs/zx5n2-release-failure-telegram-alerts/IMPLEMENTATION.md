# Release 失败 Telegram 告警接入实现

## Current Status

- Implementation: complete
- Lifecycle: implemented
- Provider: Oidrune reusable workflow
- Pinned workflow revision: `e48822f99c6402a753ed86557ea029754cbab20b`

## Coverage

- `.github/workflows/notify-release-failure.yml` 保留 `Release` 的 `workflow_run` completed/main 过滤、失败结论判定、release target SHA log resolver 和 `workflow_dispatch` smoke 入口。
- `notify_failure` 与 `smoke_test` 都调用 pinned Oidrune `notify.yml`，只传 `outcome` 与 caller-owned `summary`，并在 job 级别授予 `id-token: write`。
- failure summary 保留 `🚨 Release Failed · owner/repo` 首行，并使用 resolver 输出的 target SHA、ref、actor、run attempt、run URL 和解析详情；smoke summary 保留 `🧪 Smoke Test · owner/repo` 首行，并使用 dispatch SHA、当前 run URL、ref、actor、run attempt 和 smoke 说明。
- 旧 `github-workflows/release-failure-telegram.yml@main` 引用与 `SHOUTRRR_URL` secret 传递已移除。
- `.github/scripts/test-release-scripts.sh` 增加静态 workflow contract tests，并继续由 `CI (PR)` 与 `CI (main)` 的 `quality` job 执行。

## Validation Contract

- 本地运行 `bash ./.github/scripts/test-release-scripts.sh`，覆盖 release scripts 与 notification workflow contract。
- CI 继续运行既有 `cargo fmt`、clippy、Rust、mtproto-helper 和 macOS Swift gates。
- 本任务不执行真实 `workflow_dispatch` smoke notification；该入口只由授权维护者在 GitHub 上手动验证。

## References

- `./SPEC.md`
- `./HISTORY.md`
- [`Oidrune notify.yml at the pinned commit`](https://github.com/IvanLi-CN/oidrune/blob/e48822f99c6402a753ed86557ea029754cbab20b/.github/workflows/notify.yml)
