# History

## Durable Rationale

- APFS local snapshots are used as a read-only filesystem view because live-file scanning cannot guarantee one backup time point.
- The privileged component remains a narrow mount and lifecycle proxy so user Keychain and backup data stay in the user daemon.
- The file-reading process remains an independent FDA identity, but is delivered inside the single
  visible product app. Official ad-hoc registration uses the canonical product `Program` path because
  macOS reserves `BundleProgram` for `SMAppService`; its unchanged artifact is reused across ordinary
  releases, while helper code changes remain explicit authorization migrations.
