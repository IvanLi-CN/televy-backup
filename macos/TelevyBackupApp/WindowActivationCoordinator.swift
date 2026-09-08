import AppKit
import Foundation

enum PersistentWindowActivationSource: Equatable {
    case appSwitcher
    case dock
    case statusItem
}

enum PersistentWindowApplicationPolicy: Equatable {
    case accessory
    case regular
}

struct PersistentWindowActivationState {
    private struct WindowState: Equatable {
        var isOpen = false
        var isMinimized = false
    }

    private var windows: [String: WindowState] = [:]
    private(set) var lastKeyWindowID: String?

    var applicationPolicy: PersistentWindowApplicationPolicy {
        windows.values.contains { $0.isOpen || $0.isMinimized } ? .regular : .accessory
    }

    var hasOpenWindows: Bool {
        windows.values.contains { $0.isOpen || $0.isMinimized }
    }

    var hasVisibleWindows: Bool {
        windows.values.contains { $0.isOpen && !$0.isMinimized }
    }

    mutating func register(_ id: String) {
        if windows[id] == nil {
            windows[id] = WindowState()
        }
    }

    mutating func present(_ id: String) {
        register(id)
        windows[id]?.isOpen = true
        windows[id]?.isMinimized = false
    }

    mutating func markKey(_ id: String) {
        guard let window = windows[id], window.isOpen || window.isMinimized else { return }
        lastKeyWindowID = id
    }

    mutating func minimize(_ id: String) {
        guard windows[id] != nil else { return }
        windows[id]?.isOpen = true
        windows[id]?.isMinimized = true
    }

    mutating func restore(_ id: String) {
        guard windows[id]?.isOpen == true || windows[id]?.isMinimized == true else { return }
        windows[id]?.isOpen = true
        windows[id]?.isMinimized = false
        lastKeyWindowID = id
    }

    mutating func close(_ id: String) {
        guard windows[id] != nil else { return }
        windows[id]?.isOpen = false
        windows[id]?.isMinimized = false
        if lastKeyWindowID == id {
            lastKeyWindowID = nil
        }
    }

    func restorationTarget(for source: PersistentWindowActivationSource) -> String? {
        guard source == .appSwitcher || source == .dock else { return nil }
        guard hasOpenWindows, !hasVisibleWindows else { return nil }

        if let lastKeyWindowID,
           let window = windows[lastKeyWindowID],
           window.isOpen || window.isMinimized
        {
            return lastKeyWindowID
        }
        return windows.keys.sorted().first(where: { id in
            guard let window = windows[id] else { return false }
            return window.isOpen || window.isMinimized
        })
    }
}

final class PersistentWindowActivationCoordinator: NSObject, NSWindowDelegate {
    private let application: NSApplication
    private let closePopover: () -> Void
    private let reportFailure: (String) -> Void
    private var state = PersistentWindowActivationState()
    private var windowsByObjectID: [ObjectIdentifier: String] = [:]
    private var windowsByID: [String: NSWindow] = [:]
    private var statusItemActivationPending = false
    private var policyReconciliationScheduled = false

    init(
        application: NSApplication = NSApp,
        closePopover: @escaping () -> Void,
        reportFailure: @escaping (String) -> Void
    ) {
        self.application = application
        self.closePopover = closePopover
        self.reportFailure = reportFailure
    }

    func present(_ window: NSWindow, id: String) {
        register(window, id: id)
        state.present(id)
        closePopover()
        applyPolicyIfNeeded()

        application.activate(ignoringOtherApps: true)
        if window.isMiniaturized {
            window.deminiaturize(nil)
        }
        window.makeKeyAndOrderFront(nil)
        state.markKey(id)
    }

    func register(_ window: NSWindow, id: String) {
        state.register(id)
        windowsByObjectID[ObjectIdentifier(window)] = id
        windowsByID[id] = window
        window.delegate = self
    }

    func noteStatusItemActivation() {
        statusItemActivationPending = true
        DispatchQueue.main.async { [weak self] in
            guard let self, self.statusItemActivationPending else { return }
            self.statusItemActivationPending = false
        }
    }

    func applicationDidBecomeActive() {
        guard !statusItemActivationPending else {
            statusItemActivationPending = false
            return
        }
        restoreMostRecentWindow(for: .appSwitcher)
    }

    func handleDockReopen(hasVisibleWindows: Bool) -> Bool {
        if restoreMostRecentWindow(for: .dock) {
            return true
        }
        if hasVisibleWindows || state.hasOpenWindows {
            return true
        }
        return false
    }

    @discardableResult
    func restoreMostRecentWindow(for source: PersistentWindowActivationSource) -> Bool {
        guard let id = state.restorationTarget(for: source),
              let window = windowsByID[id]
        else {
            return false
        }

        state.restore(id)
        applyPolicyIfNeeded()
        application.activate(ignoringOtherApps: true)
        if window.isMiniaturized {
            window.deminiaturize(nil)
        }
        window.makeKeyAndOrderFront(nil)
        return true
    }

    func windowDidBecomeKey(_ notification: Notification) {
        guard let window = notification.object as? NSWindow,
              let id = windowsByObjectID[ObjectIdentifier(window)]
        else { return }
        state.present(id)
        state.markKey(id)
        applyPolicyIfNeeded()
    }

    func windowDidMiniaturize(_ notification: Notification) {
        guard let window = notification.object as? NSWindow,
              let id = windowsByObjectID[ObjectIdentifier(window)]
        else { return }
        state.minimize(id)
        applyPolicyIfNeeded()
    }

    func windowDidDeminiaturize(_ notification: Notification) {
        guard let window = notification.object as? NSWindow,
              let id = windowsByObjectID[ObjectIdentifier(window)]
        else { return }
        state.present(id)
        applyPolicyIfNeeded()
    }

    func windowWillClose(_ notification: Notification) {
        guard let window = notification.object as? NSWindow,
              let id = windowsByObjectID[ObjectIdentifier(window)]
        else { return }
        state.close(id)
        schedulePolicyReconciliation()
    }

    private func schedulePolicyReconciliation() {
        guard !policyReconciliationScheduled else { return }
        policyReconciliationScheduled = true
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.policyReconciliationScheduled = false
            self.applyPolicyIfNeeded()
        }
    }

    private func applyPolicyIfNeeded() {
        let desired: NSApplication.ActivationPolicy = state.applicationPolicy == .regular ? .regular : .accessory
        guard application.activationPolicy() != desired else { return }
        guard application.setActivationPolicy(desired) else {
            reportFailure(
                desired == .regular
                    ? "Failed to show TelevyBackup in Dock and Cmd-Tab"
                    : "Failed to hide TelevyBackup from Dock and Cmd-Tab"
            )
            return
        }
        guard application.activationPolicy() == desired else {
            reportFailure(
                desired == .regular
                    ? "Failed to show TelevyBackup in Dock and Cmd-Tab"
                    : "Failed to hide TelevyBackup from Dock and Cmd-Tab"
            )
            return
        }
    }
}
