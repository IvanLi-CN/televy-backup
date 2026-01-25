# IPC / RPC Contracts（daemon ↔ CLI）

## Status stream (Unix domain socket)

- Scope: internal
- Change: New
- Transport: Unix domain socket（stream）
- Producer: `televybackupd`
- Consumer: `televybackup status stream`（CLI）
- Payload: NDJSON 行（每行一个 `StatusSnapshot`，形状见 `contracts/events.md` 的 `status.snapshot`）

### Socket location

- 默认：`$TELEVYBACKUP_DATA_DIR/ipc/status.sock`

### Handshake

- 无显式握手：客户端连接后，daemon 立即开始输出 NDJSON。
- 连接建立后第一条必须在 `≤ 500ms` 内发送。

### Cadence

- running: `5–10Hz`（建议上限 10Hz）
- idle: `1Hz`

### Why NDJSON

- 一行一个 JSON：简化 framing 与增量解析；不需要 length-prefix。
- 便于调试：可直接观察/截取单行复现。

### Security

- 仅本机 socket；不监听 TCP。
- 不输出 secrets。

