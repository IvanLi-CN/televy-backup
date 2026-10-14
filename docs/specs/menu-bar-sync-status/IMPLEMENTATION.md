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
- Menu-bar idle/activity icons load the transparent monochrome template brand mark from `Brand/televybackup-logo-template.svg`. Its centered 1050-point viewBox reduces transparent padding and gives the product mark a visible size of approximately 12.1 × 9.3pt in the 18pt canvas; the DEV badge and cache key semantics remain unchanged.
- The five active states draw bare foreground symbols in the lower-right 11pt region: up arrow, down arrow, checkmark, bidirectional arrows, and a red exclamation mark. Each symbol contour is expanded by 1pt to clear the underlying product mark before the foreground is painted; no circle or triangle background is drawn.
- The status-bar button now routes left clicks to the popover and right clicks to a fixed quick-action menu. Its backup/stop availability reuses `TargetPresentation` and the active-task contract; GUI-only and complete exits share the lifecycle gate and keep daemon ownership explicit.
- The deterministic quick-action preview captures the same six-item menu in light and dark appearances; lifecycle integration exercises the matching GUI-only and complete-exit ownership boundaries without reusing daemon IPC as a GUI control channel.

## Validation Status

- Rust and Swift focused tests cover the status contract, live failure lifecycle, task mutual exclusion, rate preference, status icon rendering, and quick-action backup gating; the lifecycle integration script covers both exit modes.
- Controlled AppKit previews cover both Release and Dev status icons in light and dark appearances. The owner confirmed the complete matrix with bare status symbols and transparent contour clearance; the accepted Release/Dev previews and complete light/dark matrix are saved as canonical visual evidence.

The Popover title uses `BrandMark.swift` to select the transparent light or dark SVG inside the existing 28pt header tile.

## Related Changes

- None

## References

- `./SPEC.md`
- `./HISTORY.md`
