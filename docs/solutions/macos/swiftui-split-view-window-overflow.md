---
title: Diagnose SwiftUI Split View Content Above the Window
module: macos-app
problem_type: rendering-layout
component: main-window
tags:
  - macos
  - swiftui
  - appkit
  - layout
  - regression-testing
status: active
related_specs:
  - docs/specs/targets-runs-main-window/SPEC.md
---

# Diagnose SwiftUI Split View Content Above the Window

## Context

A resizable AppKit window hosts a SwiftUI NavigationSplitView. The detail header gains recovery instructions when a running backup fails, with a separately scrolling history list below it.

## Symptoms

After the terminal update, the sidebar heading and detail header move above the visible window. Scrolling history cannot recover them because the overflow belongs to the outer layout. Enlarging the window can reveal content. Rendering the recovery notice alone does not reproduce the problem.

## Root cause

The recovery Text used `fixedSize(horizontal: false, vertical: true)`. During split-view sizing, its vertical ideal-size request could expand the outer layout beyond the viewport. Native geometry reproduced the problem with only one history row; changing hosting-controller sizing options or allowing the history list to shrink did not correct it. Removing only the text's vertical fixed-size rule restored the sidebar and detail bounds.

## Resolution

Let recovery text accept the split view's layout proposal. Preserve the instructions, recovery actions and independent history scrolling. Verify the actual production window rather than replacing the layout with a screenshot-only component wrapper.

Align AppKit's `contentMinSize` with the SwiftUI root's minimum content size. Setting the same values through `minSize` reserves them for the entire window, allowing window chrome to reduce the usable content height and clip headings at the native minimum frame.

## Guardrails / Reuse notes

- Drive the real NSWindow creation path, a running-to-failed status update and both short and overflowing synthetic history.
- Measure critical elements in window coordinates against `contentLayoutRect`; checking that a view exists or rendering an isolated component does not prove visibility.
- Test scrolling to both ends, the native minimum window frame, enlargement/restoration and tab changes. Both columns must remain inside the viewport.
- Use the normal AppKit application event loop so SwiftUI display transactions run. Keep geometry anchors behind `TELEVYBACKUP_TESTING` and remove throwaway rendering controls.
- Isolate fixture stores from asynchronous runtime log-refresh callbacks before measuring; otherwise history may disappear and produce a false pass.
- Pin config/data/log environment overrides to the harness's own temporary directory and disable Keychain. Demo defaults alone do not isolate an inherited explicit directory override; reject mismatched paths before model initialization.
- Run the native harness on GitHub-hosted macOS with synthetic data. Capture only its exact window ID, without desktop capture, installed apps, backup data, FDA or root helpers.

## References

- [Main-window contract](../../specs/targets-runs-main-window/SPEC.md)
- `scripts/macos/test-main-window-layout.sh`
- `macos/TelevyBackupAppTests/MainWindowLayoutTests.swift`
