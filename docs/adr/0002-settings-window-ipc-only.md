# Settings Window Uses Control IPC Only

## Context

The macOS Settings window previously launched the bundled CLI for daemon-backed reads and
writes. The CLI was an independent process and could be terminated by an external supervisor,
which surfaced as `signal=9` in the window even when the daemon itself was healthy. Reusing an
`NSWindow` also meant that closing and reopening it did not reliably trigger a fresh load.

## Decision

All daemon business calls made by `SettingsWindow` use the authenticated local control socket.
The window has no CLI subprocess path and no compatibility fallback. Short requests use bounded
deadlines and structured `ControlError` values. Secret values are sent only over the private
socket and are never written to request/response logs.

Settings reads return an opaque content revision. Writes carry the revision observed by the editor
and reject stale updates. The app keeps the last successful settings snapshot in memory only.
Reopening a reused window posts one explicit reload request; in-flight reloads are coalesced and
obsolete responses are discarded.

## Consequences

- A new app talking to an old daemon reports `control.method_not_found` as an incompatibility;
  it does not launch the CLI.
- Telegram validation, chat discovery, configuration bundle actions, restore, and backup enqueue
  are addressed as control methods and receive structured failures.
- The terminal `televybackup` CLI remains a supported product interface, but it is not an app
  transport.
- Long-running Telegram and bundle-apply work is daemon-owned and observed through `operation.get`
  behind the same method namespace; the app's process boundary does not change.

## Rejected alternatives

- Launching the CLI after an IPC failure would preserve the failure mode and make compatibility
  behavior ambiguous.
- Persisting a settings cache across daemon restarts could hide stale revisions and secrets state.
