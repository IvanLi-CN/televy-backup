# History

## Surviving Access process upgrades

- Published rc.114 and rc.120 Access artifacts both advertised component version `0.2.0` despite expecting mount protocols 1 and 4. On a disposable Actions runner, updating files at the same paths and successfully installing the new root helper left the old Access process alive. Direct LaunchAgents omit parent bundle metadata, and duplicate bootstrap returned error 5 without the text previously required for restart recovery. Running-code validity now distinguishes the old process, registration refreshes only an idle owned service, and the installation coordinator includes Access registration before reporting communication readiness. The approved Access artifact remains unchanged, preserving its FDA identity.

## Installation feedback

- PR #176 follows the protocol-compatibility release with a hosted regression for the privileged installer and its UI feedback. A status refresh previously overwrote administrator installation failures; installation and Access communication now form one completion result, and the CLI verifies the new service before acknowledging readiness. The reproduced feedback failure and the passing published-artifact installation are distinct evidence; neither establishes the cause of an owner's inaccessible runtime state.

## Durable Rationale

- APFS local snapshots are used as a read-only filesystem view because live-file scanning cannot guarantee one backup time point.
- The privileged component remains a narrow mount and lifecycle proxy so user Keychain and backup data stay in the user daemon.
- The file-reading process remains an independent FDA identity, but is delivered inside the single
  visible product app. Official ad-hoc registration uses the canonical product `Program` path because
  macOS reserves `BundleProgram` for `SMAppService`; its unchanged artifact is reused across ordinary
  releases, while helper code changes remain explicit authorization migrations.
