# Settings Control IPC

## Goals

- Provide one versioned control-socket contract for settings, diagnostics, secrets, Telegram,
  configuration bundles, restore, backup enqueue, and operation status.
- Keep settings writes revision-aware and prevent stale editors from overwriting newer state.
- Keep secret payloads local to the authenticated socket and out of logs.
- Let the macOS Settings window present unavailable, timeout, incompatibility, vault, conflict,
  and operation failures without launching a child CLI.

## Non-goals

- Removing or changing the user-facing terminal CLI.
- Persisting a second settings cache in the app.
- Forcing a daemon restart or cancelling a backup as a side effect of a settings failure.

## Contract

Requests are newline-delimited JSON envelopes:

```json
{"type":"control.request","id":"<opaque request id>","method":"settings.get","params":{}}
```

Responses echo the request id and contain either `result` or a structured `error` with `code`,
`message`, `retryable`, and optional non-secret `details`.

The settings methods are:

- `settings.get` -> `{settings, secrets, secretsError, revision}`
- `settings.set` with `{settings, expectedRevision}` returns `{operationId}`; poll `operation.get`
  for the terminal `{revision}` result.
- `settings.bundle.export`, `settings.bundle.inspect`, `settings.bundle.compareFolder`, and
  `settings.bundle.apply`
- `diagnostics.get`, `diagnostics.setLogLevel`, and `diagnostics.setLogRetention`
- `secrets.setTelegramBotToken`, `secrets.setTelegramApiHash`, and
  `secrets.clearTelegramMtprotoSession`
- `telegram.validate` and `telegram.waitChat`
- existing `restore.latest` and `backup.enqueue` control methods

`settings.set` and bundle apply reject stale revisions with `settings.revision_conflict` and do
not partially commit settings. The socket directory is mode `0700` and the socket is mode `0600`.
Deadlines are bounded per request; the app may explicitly retry after a visible failure but does
not retry automatically.

Long operations return `{operationId}` within the short request deadline. The daemon owns the
work and retains an in-memory status record with `state` (`pending`, `running`, `succeeded`, or
`failed`), optional `progress`, terminal `result`, and structured `error`. The app polls
`operation.get` only for an operation it explicitly started; polling expiry is reported as a
timeout and never starts a second operation.
Operation records are bounded in memory and are intentionally not a durable task queue; a daemon
restart ends in-flight operations and the app reports the resulting unavailable/timeout state.
The daemon retains at most 256 records, evicting terminal records only; if all retained records
are active it rejects a new operation with `operation.capacity`.
Configuration bundle writes are the exception: their encrypted rollback marker is recovered before
the daemon loads settings.

## Compatibility

An unsupported method returns `control.method_not_found`. The app renders this as an incompatible
daemon error and never falls back to a CLI subprocess.

## Related ADRs

- [0002-settings-window-ipc-only](../../adr/0002-settings-window-ipc-only.md)
