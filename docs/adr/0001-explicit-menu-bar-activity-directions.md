# Use explicit activity directions for menu-bar state

The macOS menu bar derives its global activity state from explicit task kind and transfer-direction fields in `StatusSnapshot`, rather than inferring them from phase names or instantaneous transfer rates. This preserves correct Backup, Restore, Verify, and Bidirectional Sync states during preparation, zero-rate intervals, concurrent work, and future native sync operations.

## Considered Options

- Infer the state from `phase`, `up`, and `down`; this loses task intent and misclassifies preparation and zero-rate intervals.
- Expose task kind and declared directions through the status contract; this adds a small, backward-compatible protocol surface and makes the UI projection deterministic.
