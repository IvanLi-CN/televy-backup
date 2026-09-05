# 接口契约（Contracts）

本目录用于存放本计划的**接口契约**。为避免形状混杂，契约按 `Kind` 拆分（不要把 Config/CLI/DB 混在一个文件里）。

编写约定：

- `../PLAN.md` 是唯一的“接口清单（Inventory）”：每条接口都必须在那张表里出现。
- 在 `../PLAN.md` 的 `Contract Doc` 列，填入对应契约文件的相对路径（例如 `./contracts/config.md`）。
- 修改既有接口时，契约里必须写清楚：
  - 变化点（旧 → 新）
  - 向后兼容期望
  - 迁移 / rollout 方案（若需要）

本计划包含：

- `config.md`：`telegram.mode=mtproto` 与 MTProto 必要配置/secret key 的形状
- `cli.md`：`televybackup telegram validate` 在 mtproto 模式下的验证口径
- `db.md`：`tgmtproto:` object_id 编码方案与兼容策略
- `file-formats.md`：Keychain vault key + 本地加密 secrets store（`secrets.enc`）与迁移策略
