# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## 文档更新（Docs to Update）

- `docs/specs/index-tiered-filemaps/SPEC.md`: 本规格
- `docs/specs/README.md`: Index 表新增条目并跟踪状态


## 计划资产（Plan assets）

None


## 资产晋升（Asset promotion）

None


## 方案概述（Approach, high-level）

- remote index 文件的“爆炸”来自历史快照的 `files` / `file_chunks` 重复存储；但运行时关键路径（base-chunk-copy）只需要每个 source 的 latest 快照映射。
- 因此在上传前进行 export：把“快照目录与远端指针（snapshots/remote_indexes）”保留为全量，把“文件映射（files/file_chunks）”裁剪为每个 source 的 latest（去重映射在 #3z7rj 中迁移到 dedupe DB）。
- 每个旧快照在其自身 remote index 中仍包含其文件映射（因为当时它是 latest），所以 restore 仍可按 manifest 单独下载。


## 变更记录（Change log）

- 2026-03-02: 创建规格
