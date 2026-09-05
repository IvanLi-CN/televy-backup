# Implementation

## Current State

- Legacy plan status: `已完成`.
- Canonical implementation state: `implemented`.

## Migrated Delivery Notes

## 非功能性验收 / 质量门槛（Quality Gates）

### Testing

- Unit tests: CDC 切分稳定性、hash 计算、加密/解密 round-trip、SQLite 基础 CRUD 与约束。
- Integration tests: 使用“本地假存储（in-memory / fs mock）”模拟上传/下载，跑一次端到端备份→恢复→校验。
- E2E tests (required): 必须覆盖 UI 基本流程（设置 → 发起任务 → 展示进度 → 展示结果/错误）。
  - 本计划不引入 UI 自动化；以“macOS 手工冒烟清单”作为 E2E 验收口径。

### macOS 手工冒烟清单（required）

- 安装并启动 App（首次启动无崩溃）
- Settings：写入 bot token（Keychain）与 chat_id，点击 Validate，结果为成功
- Backup：选择一个小目录（含多个文件），启动备份，能看到 progress/state，最终 succeeded
- Restore：选择该 snapshot，恢复到空目录，最终 succeeded 且文件数量/大小一致
- Verify：对该 snapshot 执行 verify，最终 succeeded

### Quality checks

- Rust: `cargo fmt` / `cargo clippy -D warnings` / `cargo test`


## 里程碑（Milestones）

- [x] M1: 冻结 Telegram 存储路径与鉴权方案（Bot API + 私聊 Bot）并在契约中固化
- [x] M2: 冻结 SQLite schema 与索引上传策略（索引加密分片上传 + manifest）
- [x] M3: 备份管线 MVP（scan → chunk → encrypt → upload → index）
- [x] M4: 恢复/校验 MVP（fetch index → fetch chunks → reassemble → verify）
- [x] M5: UI MVP（native macOS：任务列表/进度/错误/统计/基础设置）
- [x] M6: 调度与保留策略（小时/天触发；GC/保留）
- [x] M7: 打包与发布（brew 安装 + `brew services` 管理 + 升级不丢数据）

## Migration State

- Canonical topic established from the legacy plan.
- Legacy source retained pending delete approval: `docs/plan/0001:telegram-backup-mvp/PLAN.md`.
