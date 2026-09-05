# 接口契约（Contracts）

本目录用于存放 `sync-logging-durability` 主题的接口契约。为避免形状混杂，契约必须按 `Kind` **拆分成不同文件**（不要把 Config/File 等混在一个文件里）。

编写约定：

- 只保留本计划实际用到的契约文件（不用的不要创建/应删除）。
- `../SPEC.md` 是主题级需求与验收入口；每条接口都必须能从主题文档追溯。
- 契约文件使用相对路径链接到同主题的 `SPEC.md`、`IMPLEMENTATION.md` 或
  `HISTORY.md`，不要重新指向 legacy Plan。
- 修改既有接口时，契约里必须写清楚：
  - 变化点（旧 → 新）
  - 向后兼容期望
  - 迁移 / rollout 方案（若需要）

本主题包含：

- `config.md`：日志相关环境变量（internal）
- `file-formats.md`：每轮同步日志文件的目录/命名/格式（internal）
