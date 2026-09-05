# Daemon Lifecycle RPC

## `daemon.stop`

- Request: `ControlRequest { method: "daemon.stop", params: {} }`.
- Response: acknowledgement that shutdown has been requested.
- Behavior: daemon cancels the active scheduled task, waits for normal task cleanup, closes IPC services, releases the instance lock, and exits.
- The caller observes completion by waiting for the daemon IPC socket to become unavailable; an acknowledgement is not completion.
