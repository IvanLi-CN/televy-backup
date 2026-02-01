# Config contracts（Settings v2）

> Kind: Config（internal）

## `SettingsV2.telegram_endpoints[*].chat_id` 必须全局唯一

- 范围（Scope）: internal
- 变更（Change）: Modify

### Rule

- 对所有 endpoints，`chat_id` 组成集合必须无重复。

### Failure

- Error: `config.invalid`
- Message: `telegram_endpoints[].chat_id must be unique (chat_id=<value> is reused)`

