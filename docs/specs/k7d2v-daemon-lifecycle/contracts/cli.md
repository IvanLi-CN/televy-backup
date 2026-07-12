# Daemon Lifecycle CLI

- `televybackup daemon start`: start a background daemon for the selected config/data directories and return only after IPC is ready.
- `televybackup daemon status`: report whether daemon IPC is reachable.
- `televybackup daemon stop`: request `daemon.stop` and wait up to ten seconds for IPC shutdown.

Only CLI-owned temporary daemons are cleaned up automatically at command completion. Existing daemons remain shared and untouched.
