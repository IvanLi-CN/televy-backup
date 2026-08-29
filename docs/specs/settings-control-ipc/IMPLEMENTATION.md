# Implementation

- Core defines typed settings, bundle, Telegram, and revision contracts in `control.rs`.
- The daemon serves settings and diagnostics over the private control socket and redacts secret
  values from structured errors.
- Settings writes, Telegram validation/chat discovery, and bundle apply are daemon-owned
  operations. Stale settings revisions are rejected before an operation starts, and the actual
  write is observed through `operation.get` so a client deadline cannot hide a late commit.
- The macOS app shares `ControlIPCClient` between settings and snapshot inspection.
- `SettingsWindow` contains no CLI subprocess invocation and coalesces reloads when a reused
  window is shown again.
