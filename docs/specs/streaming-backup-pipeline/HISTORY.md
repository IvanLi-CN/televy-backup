# History

## Provenance

- Existing ID-prefixed Spec normalized to this slug-only topic.

## Durable Rationale and Change Record

## 变更记录（Change log）

- 2026-02-26: 新建规格，冻结并行主流水线与进度语义修复目标。
- 2026-02-26: 主循环改为“扫描发现缺失即入队上传”，并补齐 core/cli/daemon/swift 单测验证。
- 2026-02-27: retention preflight 改为批处理删除并加入批次日志，新增 migration `0003_retention_perf.sql` 调整索引，`televy_backup_core` 回归测试通过。
- 2026-02-27: 为 index part + manifest 上传补齐重试/退避（与数据块上传一致），避免单次 MTProto 45s 超时直接导致整轮失败；补充 `pack_uploads` 重试用例。
- 2026-02-28: `upload_index` 改为流式压缩上传（临时文件分片读取），移除整库读入内存路径；新增大索引多分片上传测试 `large_index_db_uploads_multiple_index_parts`。
