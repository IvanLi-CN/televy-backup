# History

## Installation feedback

- PR #176 follows the protocol-compatibility release with a hosted regression for the privileged installer and its UI feedback. A status refresh previously overwrote administrator installation failures; installation and Access communication now form one completion result, and the CLI verifies the new service before acknowledging readiness. The reproduced feedback failure and the passing published-artifact installation are distinct evidence; neither establishes the cause of an owner's inaccessible runtime state.

## Durable Rationale

- APFS local snapshots are used as a read-only filesystem view because live-file scanning cannot guarantee one backup time point.
- The privileged component remains a narrow mount and lifecycle proxy so user Keychain and backup data stay in the user daemon.
- The file-reading process remains an independent FDA identity, but is delivered inside the single
  visible product app. Official ad-hoc registration uses the canonical product `Program` path because
  macOS reserves `BundleProgram` for `SMAppService`; its unchanged artifact is reused across ordinary
  releases, while helper code changes remain explicit authorization migrations.
