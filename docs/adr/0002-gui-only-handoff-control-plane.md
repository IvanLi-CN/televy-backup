# Use a dedicated GUI control plane for GUI-only handoff

`televybackup gui quit` targets one data directory through a private, versioned GUI socket and succeeds only after the GUI records a stopped lease and releases its lifecycle lock. Reusing daemon control IPC would blur daemon and GUI ownership, while process scanning or signals cannot prove environment ownership and would make GUI updates unsafe.

## Considered Options

- Reuse daemon control IPC: rejected because a GUI-only exit must never alter daemon state.
- Scan, signal, or automate processes: rejected because the CLI cannot safely prove a target GUI belongs to the requested data directory.
