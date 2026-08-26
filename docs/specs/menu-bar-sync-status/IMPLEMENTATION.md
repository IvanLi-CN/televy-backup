# macOS 菜单栏同步状态与传输速率实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖与 rollout 相关事实。

## Current Status

- Implementation: complete
- Lifecycle: ready for validation
- Catalog note: active-task status contract and menu-bar projection are implemented together.

## Coverage / rollout summary

- daemon status snapshot、control IPC、CLI task events and macOS menu bar share the `activeTask` contract.
- The menu-bar rate preference is machine-local and excluded from portable configuration.

## Validation Status

- Rust and Swift focused tests cover the status contract, live failure lifecycle, task mutual exclusion, rate preference, and status icon rendering.
- Controlled AppKit previews cover both Release and Dev status icons; the owner has confirmed the final icon treatment.

## Related Changes

- None

## References

- `./SPEC.md`
- `./HISTORY.md`
