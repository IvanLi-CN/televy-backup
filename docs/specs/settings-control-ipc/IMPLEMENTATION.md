# Implementation

- Core defines typed settings, bundle, Telegram, and revision contracts in `control.rs`.
- The daemon serves settings and diagnostics over the private control socket and redacts secret
  values from structured errors.
- Telegram validation/chat discovery and bundle apply are daemon-owned operations. They return an
  operation id immediately and are observed through `operation.get` until a terminal result.
- The macOS app shares `ControlIPCClient` between settings and snapshot inspection.
- `SettingsWindow` contains no CLI subprocess invocation and coalesces reloads when a reused
  window is shown again.
