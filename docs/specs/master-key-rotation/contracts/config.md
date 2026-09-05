# Config / secrets contracts（rotation）

> Kind: Config（internal）

## Secrets store keys（rotation）

轮换期间需要在 secrets store（`secrets.enc`）中临时持久化 pending key 与 rotation state 引用。

### Keys

- Active master key（existing, unchanged）：`televybackup.master_key`
- Pending master key（new, rotation-only）：`televybackup.master_key.next`
  - Value: Base64(32 bytes)
- Rotation metadata pointer（non-secret）：`televybackup.master_key.rotation_state`
  - Value: JSON（不含 master key 明文；用于指向 rotation state 文件）

```json
{
  "version": 1,
  "path": "TELEVYBACKUP_CONFIG_DIR/rotation/master-key.json"
}
```

### Rules (frozen)

- `televybackup.master_key` 在轮换完成前不得被修改。
- `televybackup.master_key.next` 仅允许被轮换任务读取；普通 backup/restore/verify 必须继续使用 active master key。
- `cancel` 必须删除 `televybackup.master_key.next` 并清理 `televybackup.master_key.rotation_state`。
- `commit` 必须以原子语义完成切换：
  - 将 `televybackup.master_key.next` 的值写入 `televybackup.master_key`
  - 删除 `televybackup.master_key.next`
  - 删除 `televybackup.master_key.rotation_state`
