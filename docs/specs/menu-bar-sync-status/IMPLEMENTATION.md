# macOS 菜单栏同步状态与传输速率实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖与 rollout 相关事实。

## Current Status

- Implementation: complete
- Lifecycle: PR convergence repair
- Catalog note: active-task status contract and menu-bar projection are implemented together.

## Coverage / rollout summary

- daemon status snapshot、control IPC、CLI task events and macOS menu bar share the `activeTask` contract.
- The menu-bar rate preference is machine-local and excluded from portable configuration.
- External terminal status now requires a matching applied transition or an exact in-memory replay acknowledgement; stale ownership and daemon restart do not create a false acknowledgement.
- Generic restore and verify resolve a unique configured target before admission, while the menu-bar failure latch deduplicates only the same task identity.
- Menu-bar icon assignment is keyed and cached. Repeated `effectiveAppearance` notifications no longer reassign the same image, while failure icons retain one cached rendering per light and dark treatment.
- The status-bar button now routes left clicks to the popover and right clicks to a fixed quick-action menu. Its backup/stop availability reuses `TargetPresentation` and the active-task contract; GUI-only and complete exits share the lifecycle gate and keep daemon ownership explicit.
- The deterministic quick-action preview captures the same six-item menu in light and dark appearances; lifecycle integration exercises the matching GUI-only and complete-exit ownership boundaries without reusing daemon IPC as a GUI control channel.

## Validation Status

- Rust and Swift focused tests cover the status contract, live failure lifecycle, task mutual exclusion, rate preference, status icon rendering, and quick-action backup gating; the lifecycle integration script covers both exit modes.
- Controlled AppKit previews cover both Release and Dev status icons; the owner has confirmed the final icon treatment.

## Related Changes

- None

## References

- `./SPEC.md`
- `./HISTORY.md`
