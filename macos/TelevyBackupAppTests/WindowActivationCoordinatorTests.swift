import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func testPolicyLifecycle() {
    var state = PersistentWindowActivationState()
    expect(state.applicationPolicy == .accessory, "no persistent windows should use accessory policy")

    state.present("main")
    state.markKey("main")
    expect(state.applicationPolicy == .regular, "presenting the main window should use regular policy")
    expect(state.hasVisibleWindows, "a presented window should be visible")

    state.minimize("main")
    expect(state.applicationPolicy == .regular, "a minimized window should keep regular policy")
    expect(!state.hasVisibleWindows, "a minimized window should not count as visible")

    state.present("settings")
    state.markKey("settings")
    state.close("main")
    expect(state.applicationPolicy == .regular, "closing one window should keep regular policy for another")

    state.close("settings")
    expect(state.applicationPolicy == .accessory, "closing the last persistent window should restore accessory policy")
}

private func testRestorationTargets() {
    var state = PersistentWindowActivationState()
    state.present("main")
    state.markKey("main")
    state.minimize("main")
    state.present("settings")
    state.markKey("settings")
    state.minimize("settings")

    expect(
        state.restorationTarget(for: .appSwitcher) == "settings",
        "Cmd-Tab should restore the most recent key window"
    )
    expect(
        state.restorationTarget(for: .dock) == "settings",
        "Dock reopen should restore the most recent key window"
    )
    expect(
        state.restorationTarget(for: .statusItem) == nil,
        "status bar activation must not restore a minimized persistent window"
    )

    state.restore("settings")
    expect(
        state.restorationTarget(for: .appSwitcher) == nil,
        "app switching with a visible persistent window needs no forced restore"
    )
}

private func testExternalReopenWithoutPersistentWindow() {
    var state = PersistentWindowActivationState()
    state.register("main")
    expect(!state.hasOpenWindows, "registered but closed windows must not count as open")
    expect(
        state.restorationTarget(for: .dock) == nil,
        "external reopen without a persistent window should leave popover handling to AppDelegate"
    )
}

@main
enum WindowActivationCoordinatorTestsMain {
    static func main() {
        testPolicyLifecycle()
        testRestorationTargets()
        testExternalReopenWithoutPersistentWindow()
        print("OK: WindowActivationCoordinatorTests")
    }
}
