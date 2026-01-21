# File Formats Contracts（remote bootstrap/catalog）

> Kind: File format（internal）

## 1) 目标

在 Telegram Bot API 的约束下（无法枚举历史文件），为跨设备恢复提供“可发现的引导信息”：

- 新设备无旧 SQLite 时，仍能定位到 latest 快照的 `snapshot_id + manifest_object_id`
- 用户只需：金钥 + bot token + chat_id

## 2) 存储位置与发现方式（frozen）

### 存储位置

- 每个 Telegram endpoint（`bot token + chat_id`）对应一个 bootstrap/catalog 文件。
- 该文件作为 Telegram `document` 上传到该 chat。

### 发现方式

- Root pointer 使用 **pinned message**：
  - bot 在 chat 中 pin 一条消息，其 document 即为最新的 bootstrap/catalog
  - 新设备通过 `getChat(chat_id=...)` 获取 `pinned_message.document.file_id`，再通过 `getFile` 下载

前置条件：

- 若目标 chat 为群组/超级群组/频道：bot 需要具备 pin 权限；否则视为 `bootstrap.forbidden`（需要用户授予权限或改用私聊）。
- pinned message 被用户取消/清空时：视为 `bootstrap.missing`（需要重新生成并 pin）。

## 3) 加密封装（frozen）

- bootstrap/catalog 明文为 JSON（UTF-8）
- 上传前必须使用既有 framing 加密（`encrypt_framed`）：
  - key：master key（由金钥导入/secrets store 提供；Keychain 仅存 vault key）
  - aad：固定字符串 `televy.bootstrap.catalog.v1`

## 4) 明文 JSON 形状（frozen）

```json
{
  "version": 1,
  "updated_at": "2026-01-20T00:00:00Z",
  "targets": [
    {
      "target_id": "aaa-bbb",
      "source_path": "/AAA/BBB",
      "label": "manual",
      "latest": {
        "snapshot_id": "snp_...",
        "manifest_object_id": "telegram_file_id"
      }
    }
  ]
}
```

约束：

- `manifest_object_id` 必须是 Bot API 可用于 `getFile` 的 `file_id`
- 至少需要记录 `latest`；是否保留历史（多版本）在实现阶段决定
- `source_path` / `label` 等敏感元数据在上传前已被 master key 加密（不明文暴露在 Telegram）

## 5) 更新语义（frozen）

- 当某个 target 的 backup 成功完成并产生新的 `manifest_object_id` 时：
  1. 读取现有 catalog（若不存在则初始化）
  2. 更新对应 target 的 `latest`
  3. 上传新 catalog（新的 document/file_id）
  4. 将该消息 pin 为最新 root pointer（或更新 root pointer）

失败处理（最低要求）：

- 若无权限 pin：返回 `bootstrap.forbidden` 并提示用户授予权限/改用私聊
- 若 pinned message 缺失：返回 `bootstrap.missing` 并提示先生成 bootstrap（通常在一次成功备份后自动生成）
- 若 catalog 解密失败：返回 `bootstrap.invalid` 并提示检查金钥是否匹配
