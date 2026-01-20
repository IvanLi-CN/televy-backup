# HTTP APIs（Telegram Bot API usage）

> Kind: HTTP API（external）

本文件只描述本项目实际使用/依赖的 Telegram Bot API 端点与响应字段，用于实现与测试（不复述 Telegram 全量文档）。

## 1) Upload: sendDocument（POST /sendDocument）

- 范围（Scope）: external
- 变更（Change）: Modify（在既有用法基础上增加“pack 对象上传”场景）
- 鉴权（Auth）: Bot token（`/bot<TOKEN>/...`）

### 请求（Request）

- Method: `POST`
- Params（至少）:
  - `chat_id`: string（私聊 chat id，按计划冻结口径）
  - `document`: `attach://<name>`（上传新文件）或 `file_id`（复用已有文件，如适用）
- Encoding: `multipart/form-data`（上传 pack 文件时必须）

### 响应（Response）

- Success: `Message`
  - 需要读取：`message.document.file_id`（用于后续 `getFile` 下载与持久化引用）

### 错误（Errors）

- `429 Too Many Requests`: 需要按 Telegram 返回的 retry-after（若提供）或按本项目 rate-limit 策略退避重试
- 其他网络错误：按“可重试/不可重试”分类（实现阶段细化）

## 2) Batch Upload: sendMediaGroup（POST /sendMediaGroup）

本计划不依赖 `sendMediaGroup`（不作为交付范围）；如未来需要进一步减少 HTTP 往返次数，可另立计划引入。

## 3) Download: getFile（POST /getFile）

- 范围（Scope）: external
- 变更（Change）: None（既有依赖）
- 鉴权（Auth）: Bot token

### 请求（Request）

- `file_id`: string

### 响应（Response）

- `file_path`: string（用于拼接下载 URL：`https://api.telegram.org/file/bot<TOKEN>/<file_path>`）

## 参考（References）

- https://core.telegram.org/bots/api#sending-files
- https://core.telegram.org/bots/api#senddocument
- https://core.telegram.org/bots/api#getfile
