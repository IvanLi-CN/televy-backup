# macOS 菜单栏同步状态与传输速率实现状态

> 当前有效规范仍以 `./SPEC.md` 为准；这里记录实现覆盖与 rollout 相关事实。

## Current Status

- Implementation: in progress
- Lifecycle: active
- Catalog note: active-task status contract and menu-bar projection are implemented together.

## Coverage / rollout summary

- daemon status snapshot、control IPC、CLI task events and macOS menu bar share the `activeTask` contract.
- The menu-bar rate preference is machine-local and excluded from portable configuration.

## Remaining Gaps

- Complete implementation and record validation evidence.

## Related Changes

- None

## References

- `./SPEC.md`
- `./HISTORY.md`
