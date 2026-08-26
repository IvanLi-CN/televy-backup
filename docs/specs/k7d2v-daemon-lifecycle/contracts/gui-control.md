# GUI-only Control CLI

`televybackup gui quit` requests an orderly GUI-only exit. It never starts, stops, unloads, scans, signals, or otherwise controls a daemon.

## Environment selection

- `--data-dir <path>` selects exactly that GUI environment.
- Without `--data-dir`, the command selects only the Release default data directory.
- `TELEVYBACKUP_DATA_DIR`, `TELEVYBACKUP_CONFIG_DIR`, and `--config-dir` do not change the no-argument target.

## Local protocol and lifecycle proof

- The selected environment exposes `ipc/gui.sock`, `ipc/gui.state.json`, and `ipc/gui.lock` for the current local user only. The directory is `0700`; socket, state lease, and lock are `0600`; symlinks, foreign ownership, unsafe permissions, and incompatible protocol versions are rejected.
- The request is one newline-delimited JSON object: `{"version":1,"method":"gui.quit","requestId":"opaque-id"}`. The GUI responds with the same version and request id plus `accepted`, optional `code`, and optional `message`.
- The CLI reports success only after an accepted request is followed by `state="stopped"` in the lease and an OS-observable free lifecycle lock. It waits at most ten seconds after acceptance.
- A valid stopped lease plus a free lock returns idempotent success. A running or stopping lifecycle conflict returns `gui.busy`; missing, unsafe, incompatible, or unreachable control state returns `gui.unavailable`; acceptance without the stopped-and-unlocked proof returns `gui.timeout`.

Older GUI versions that do not register this control plane are unavailable and must be exited manually.
