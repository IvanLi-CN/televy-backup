# CLI Contract：remote-first index sync（#0012）

## 目标

- 默认开启“备份前置索引对齐”：以 pinned bootstrap catalog 指向的远端 latest 索引为准。
- 提供显式开关以便离线/调试。

## 变更点

### `televybackup backup run`

- 默认行为：在 scan 前执行 `index_sync` 前置步骤：
  - 若 pinned catalog 缺失：跳过对齐（按现有逻辑）。
  - 若 pinned catalog 存在且可解密：必要时下载远端 latest 索引并原子落盘为 `TELEVYBACKUP_DATA_DIR/index/index.sqlite`。
  - 若 pinned catalog 存在但不可解密：失败（拒绝覆盖），给出可操作错误（提示导入正确 master key / TBK1）。

### Flags

- `--no-remote-index-sync`
  - 禁用备份前置索引对齐。
  - 语义：不读取 pinned catalog，不下载远端索引；按本地索引执行（无索引则按首次备份）。

## 错误语义

- `bootstrap.missing`：无 pinned catalog（可降级跳过 sync）。
- `bootstrap.decrypt_failed`：存在 pinned，但无法解密（阻断并提示导入正确 master key / TBK1）。
