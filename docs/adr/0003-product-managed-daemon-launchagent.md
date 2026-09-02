# Product-Managed Daemon LaunchAgent

## Context

TelevyBackup's daemon can be started by the GUI or Homebrew, but there is no product-owned, observable per-user service contract. Release packages also need a stable location for daemon and helper binaries across upgrades. A service install must not overwrite a user's unrelated configuration or silently take over a different environment.

## Decision

The product owns exactly one user LaunchAgent labeled `com.ivan.televybackup.daemon`. The CLI is the source of truth for service operations: `daemon install-service [--replace]`, `daemon uninstall-service`, and `daemon service-status`. Installation stages binaries under a versioned directory below the TelevyBackup Application Support directory, writes a plist pointing at a stable active path, and atomically activates the new version. A manifest records the bound config/data directories and hashes. Same-environment installs are idempotent; a different environment fails closed unless the user explicitly passes `--replace`. Uninstall removes only managed plist, manifest, and versioned binaries, retaining user data.

The GUI Settings Schedule page invokes this CLI contract for service operations. Daemon business calls remain on authenticated control IPC. Homebrew labels are detected for a migration warning and remain supported as a legacy fallback, but formulas are not maintained by the product release flow.

All release binaries use strict ad-hoc signing. No Apple credentials, notarization, or automatic quarantine removal is part of this decision.

## Consequences

- Service ownership, environment binding, and upgrade rollback are inspectable without scanning arbitrary processes.
- A user must explicitly opt into persistence and explicitly resolve a directory conflict.
- The active/previous retention rule bounds disk usage while allowing rollback.
- Users downloading ad-hoc builds must verify checksums and follow manual Gatekeeper guidance.
- Homebrew users can continue existing setups, but future product features target the managed service contract.

## Alternatives Rejected

- **Homebrew-only service management**: unavailable in the standalone tool archive and cannot provide a product-owned environment contract.
- **Automatic install on first launch**: surprising external state change and unsafe for existing Homebrew or custom-directory setups.
- **In-place binary replacement**: cannot guarantee rollback when launchd has an active process or an interrupted copy.
- **Developer ID/notarization**: intentionally outside the controlled ad-hoc distribution scope.

