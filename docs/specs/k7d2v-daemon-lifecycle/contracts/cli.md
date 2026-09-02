# Daemon Lifecycle CLI

- `televybackup daemon start`: start a background daemon for the selected config/data directories and return only after IPC is ready.
- `televybackup daemon status`: report whether daemon IPC is reachable.
- `televybackup daemon stop`: request `daemon.stop` and wait up to ten seconds for IPC shutdown.
- `televybackup daemon install-service [--replace]`: explicitly install or upgrade the product-managed per-user LaunchAgent. A different config/data environment fails closed unless `--replace` is supplied.
- `televybackup daemon uninstall-service`: remove only the managed LaunchAgent and managed versioned binaries; user data is retained.
- `televybackup daemon service-status`: return structured service ownership, environment, active version, and launchd state.

Only CLI-owned temporary daemons are cleaned up automatically at command completion. Existing daemons remain shared and untouched.
