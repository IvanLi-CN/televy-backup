# Backup Snapshot Inspection

> Current requirements are defined here. Implementation coverage is recorded in `./IMPLEMENTATION.md`, topic-local compatibility and background in `./HISTORY.md`, and durable rationale in the related ADR.

## Background

The Main Window groups run-log summaries by target, but a row cannot currently answer which files a successful backup captured, how its file tree differs from the backup's direct baseline, or which logical backup blocks it references. Run logs alone cannot provide that evidence; the snapshot filemap is the authority.

## Goals

- Let an operator open a backup run from target history and inspect its result without leaving the Main Window.
- Show a successful retained snapshot's summary, file list, direct-baseline file-tree changes, and deduplicated logical backup blocks.
- Keep large snapshot inspection responsive through background paging, lazy tree expansion, search, and virtualized rows.
- Preserve the existing macOS native, dense, light/dark adaptive visual language.

## Non-goals

- File-content, text, binary, or side-by-side diffs.
- Move or rename detection; a path change remains a deletion plus an addition.
- Listing ignored paths, physical upload attempts, Telegram messages, or pack objects.
- Changing snapshot retention, restoring a selected historical snapshot, or retaining an independent permanent full-file history.

## Scope

### In scope

- A run-detail route inside the Main Window, entered by activating a history row and exited through native back navigation.
- A read-only snapshot inspector exposed through the CLI's JSON contract for terminal users and through daemon control IPC for the macOS App.
- On-demand loading of a retained snapshot filemap, with compatibility for current two-level indexes and legacy single-index snapshots.
- Tree and list presentations, a changes-only filter, and a logical-block presentation.
- Explicit loading, empty, unavailable, and error states.

### Out of scope

- Changing the backup scanner, chunking, index format, remote retention, or run-log retention.
- Direct macOS App reads of endpoint/filemap SQLite databases.
- A new window, modal inspector, or a visual-system redesign.

## Related ADRs

- [0001-snapshot-inspection-retention](../../adr/0001-snapshot-inspection-retention.md)

## Requirements

### MUST

- A history row for any run opens a detail page. Failed, cancelled, and running backups show run summary and available error/log information but must not show fabricated file or block data.
- A successful backup with a retained snapshot shows `Summary`, `Files`, and `Blocks` views. Summary is the default view; Files defaults to tree presentation with changes-only enabled.
- The Files view supports a tree and a flat list presentation, path search, and an all-files/changes-only switch. The selected presentation and filter remain stable while the detail page is open.
- Changes compare only the snapshot's `base_snapshot_id`. A first snapshot marks every stored entry as added. If the direct baseline is unavailable, the snapshot remains browseable but changes-only is disabled with an explanatory state.
- Changes are exactly `added`, `deleted`, or `changed`. A regular file is changed when its kind, size, modification time, or mode differs. A directory or symlink can only be compared by presence or kind. No move state is emitted.
- A changes-only tree contains the ancestor directories needed to reach a changed entry and provides per-directory added/deleted/changed totals. A changes-only list contains only direct change entries.
- Deleted entries are displayed at their baseline path with baseline metadata. Changed entries expose current and baseline metadata without exposing file contents.
- The Blocks view lists distinct logical blocks referenced by regular files in the snapshot, with hash, size, and referencing-file count. It does not classify a block as newly uploaded or reused in the run.
- The inspector loads data outside the main thread and presents visible loading or retryable error feedback. It must use bounded, cursor-based data access and virtualized UI rows rather than materializing a whole snapshot in SwiftUI.
- File paths, block hashes, and filemap contents stay local to the configured storage/cache path and must not be written to normal run logs or status snapshots.

### SHOULD

- Summary combines the run-log fields already available to the App (outcome, start/end, duration, transfer bytes, deduped bytes, error) with snapshot-derived counts, direct-baseline availability, source path, and snapshot ID.
- Summary exposes copy actions for the snapshot ID and an action to reveal the run log.
- File rows use a consistent SF Symbol, a visible status label, and semantic state color so color alone never communicates the change type.
- All table/tree controls are keyboard reachable and their selected state, loading state, and unavailable state are available to VoiceOver.

### COULD

- A block selection can later reveal the files referencing that block through a separate paged query.

## Functional Behavior

### Core flows

1. The App activates a target-scoped run-history row and navigates the detail pane to that run.
2. The App immediately renders run-log information. For an eligible successful run, it requests snapshot summary in the background.
3. The inspector resolves the snapshot from the retained endpoint index and opens its local filemap, downloading the snapshot filemap through the existing index resolver only when it is absent locally.
4. Summary returns snapshot totals and direct-baseline availability. Files and Blocks fetch bounded pages only after their view is selected.
5. Files in tree mode request direct children as folders expand. List mode requests a flat cursor page. Changes-only requests change rows and required ancestor context.
6. The App renders result rows through a virtualized table/tree surface and cancels or discards obsolete page work when the user changes run, presentation, query, or filter.

### Edge cases and errors

- A run without `snapshot_id`, a failed/cancelled run, or an expired snapshot presents the summary state only; it never substitutes a current/latest snapshot.
- A retained first snapshot has no baseline and reports all entries as added rather than `baseline unavailable`.
- A retained snapshot whose direct baseline is no longer retained offers all-files browsing but no calculated difference view.
- Missing, corrupted, undecryptable, or unreachable filemaps show a retryable inspection error without changing backup/restore state.
- Empty snapshots, empty block sets, and snapshots with no direct changes have distinct empty states.
- A row with legacy/missing target identity remains in the existing Unknown target grouping; it must not be reassigned by the inspector.

## Interfaces and Contracts

### Inventory

| Interface | Kind | Scope | Change | Contract | Owner | Consumers | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- |
| `snapshots inspect` | CLI JSON | internal | New | [CLI contract](./contracts/cli.md) | CLI/core | terminal users | Read-only paged snapshot inspector |
| `snapshot.inspect.summary/files/blocks` | daemon control IPC | internal | New | This specification | daemon/core | macOS App | Read-only JSON requests over the existing local authenticated control socket |
| Snapshot filemap resolver | Core API | internal | Modify | [CLI contract](./contracts/cli.md) | core | CLI, daemon | Reuses retained-snapshot materialization semantics |
| Run detail route and views | Swift API | internal | New | This specification | macOS App | Main Window | Summary, Files, Blocks |

### Contract documents

- [CLI contract](./contracts/cli.md)

## Acceptance Criteria

- Given a successful retained backup run in a target history, when its row is activated, then the Main Window navigates to its summary and offers Files and Blocks without opening a modal or separate window.
- Given a first successful snapshot, when changes-only Files is opened, then every stored file-tree entry is marked added and no baseline error is shown.
- Given a retained snapshot with added, deleted, and modified regular files, when Files is opened in tree or list presentation, then both presentations return the same direct-baseline change classification; a deleted entry remains at its former path and no move marker appears.
- Given a directory with one changed descendant, when changes-only tree mode is opened, then its ancestor directories are visible with aggregate change counts while unchanged sibling branches are absent.
- Given a snapshot whose baseline was pruned, when the run detail is opened, then all-files browsing remains available, changes-only is disabled with an explanation, and no other snapshot is used as a substitute.
- Given an expired snapshot or a failed/cancelled backup run, when its row is activated, then the App shows the execution summary and unavailable reason but does not request or display a file/block list.
- Given a snapshot containing more files or blocks than a page, when the operator scrolls, searches, expands a node, or changes view, then rows are loaded incrementally and stale work cannot overwrite the current selection.
- Given a changes-only detail that has loaded its summary, when the operator expands another directory, then the App requests the already-running daemon over its local control socket and the daemon reuses the prepared direct-baseline index rather than launching a CLI process or repeating the full comparison.
- Given a block referenced by multiple files, when Blocks is opened, then one logical block row reports the aggregate reference count rather than multiple upload-attempt rows.
- Given a legacy single-index snapshot or a current two-level snapshot, when it is retained and its filemap is available, then the inspector uses the same restored file-tree semantics as restore/verify.

## Acceptance Checklist

- [x] The durable behavior and boundaries are defined.
- [x] Loading, retention, baseline, legacy, and failure cases are covered.
- [x] Internal interfaces and their consumers are identified.
- [x] Acceptance criteria support implementation and review.

## Quality Gates

### Testing

- Rust unit tests for cursor validation, direct-baseline added/deleted/changed classification, first snapshots, unavailable baselines, duplicate block aggregation, and legacy/current filemap resolution.
- CLI JSON contract tests for summary, tree/list pages, search, empty pages, unavailable snapshots, and structured errors.
- Swift tests for eligibility, navigation, presentation/filter state, accessibility labels, cancellation of stale loads, and unavailable/error states.
- `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all-features`, `scripts/macos/swift-unit-tests.sh`, and `scripts/macos/build-app.sh`.

### UI Evidence

- Capture deterministic light and dark Main Window demo scenes for a successful detail with changes, a baseline-unavailable detail, and an unavailable failed run before declaring the UI complete.

## Visual Evidence

None before implementation.

## Related PRs

- None

## Risks and Assumptions

- Remote filemap download can be slow or fail; summary must remain usable while inspection data loads or retries.
- Snapshot filemaps can be very large; cursor semantics, virtualized rows, and bounded cache behavior are correctness requirements as well as performance requirements.
- Retention removes the metadata needed to locate old filemaps even when remote data objects remain physically present; that is an intentional inspection boundary.
- The future delta-filemap format must materialize the same immutable snapshot view before this inspector queries it.

## References

- [Two-level endpoint and snapshot indexes](../t764g-endpoint-two-level-index/SPEC.md)
- [Index tiering and historical filemap availability](../dyu56-index-tiered-filemaps/SPEC.md)
- [Run-log durability](../0003-sync-logging-durability/SPEC.md)
- [Topic implementation status](./IMPLEMENTATION.md)
- [Topic history](./HISTORY.md)
