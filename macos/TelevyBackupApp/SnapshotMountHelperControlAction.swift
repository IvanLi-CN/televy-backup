import Foundation

enum SnapshotMountHelperControlAction {
    static let reconcileInventoryMethod = "snapshot.mount_helper.reconcile_inventory"
    static let repairLegacyMethod = "snapshot.mount_helper.repair_legacy"

    static func reconcileInventory(using send: @escaping (String) -> Void) -> () -> Void {
        {
            send(reconcileInventoryMethod)
        }
    }

    static func repairLegacy(using send: @escaping (String) -> Void) -> () -> Void {
        {
            send(repairLegacyMethod)
        }
    }
}

/// An accepted daemon operation stays active until the daemon reports a terminal state.
/// A failed status request suspends observation; it does not cancel or fail the operation.
struct SnapshotMountOperationTracker {
    private(set) var label: String?
    private(set) var operationID: String?
    private(set) var isObserving = false
    var isActive: Bool { label != nil }

    mutating func start(label: String) {
        guard !isActive else { return }
        self.label = label
        operationID = nil
        isObserving = true
    }

    mutating func accept(operationID: String) {
        self.operationID = operationID
        isObserving = true
    }

    mutating func updatePhase(_ progress: SnapshotCleanupProgress) {
        guard isActive else { return }
        switch progress.stage {
        case "cleanup_retry": label = "Retrying snapshot cleanup"
        case "inventory_reconcile": label = "Reconciling snapshot inventory (\(progress.remaining_drift ?? 0) remaining)"
        default: break
        }
    }

    mutating func suspend() { isObserving = false }

    mutating func observe(state: String) {
        if state == "succeeded" || state == "failed" {
            self = Self()
        }
    }
}

/// Only the newest read may replace the visible status. Mutations invalidate older reads.
struct SnapshotStatusRefreshTracker {
    private var generation: UInt64 = 0
    private(set) var isRefreshing = false
    private(set) var lastUpdatedAt: Date?

    mutating func begin() -> UInt64 {
        generation += 1
        isRefreshing = true
        return generation
    }

    mutating func complete(_ request: UInt64, succeeded: Bool, at date: Date = Date()) -> Bool {
        guard request == generation else { return false }
        isRefreshing = false
        if succeeded { lastUpdatedAt = date }
        return true
    }

    mutating func invalidate() {
        generation += 1
        isRefreshing = false
    }
}

struct SnapshotCleanupProgress: Decodable {
    let stage: String
    let remaining_pending: UInt32?
    let remaining_drift: UInt32?
}
