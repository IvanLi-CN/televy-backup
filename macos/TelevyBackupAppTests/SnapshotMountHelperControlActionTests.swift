import Foundation

@main
enum SnapshotMountHelperControlActionTests {
    static func main() {
        var method: String?
        let action = SnapshotMountHelperControlAction.reconcileInventory { method = $0 }
        action()
        precondition(
            method == "snapshot.mount_helper.reconcile_inventory",
            "reconcile inventory action must dispatch the production ControlIPC method"
        )
        method = nil
        let repair = SnapshotMountHelperControlAction.repairLegacy { method = $0 }
        repair()
        precondition(
            method == "snapshot.mount_helper.repair_legacy",
            "legacy repair action must dispatch the production ControlIPC method"
        )
        var tracker = SnapshotMountOperationTracker()
        tracker.start(label: "Snapshot inventory reconciliation")
        tracker.accept(operationID: "synthetic-operation")
        for _ in 0..<1800 { tracker.observe(state: "running") }
        precondition(tracker.isActive && tracker.isObserving,
                     "Long-running daemon work must never become a UI timeout failure")
        tracker.start(label: "Duplicate request")
        precondition(tracker.operationID == "synthetic-operation",
                     "An active operation cannot be replaced by a duplicate start")
        tracker.suspend()
        precondition(tracker.isActive && !tracker.isObserving,
                     "A disconnected status request must keep mutations blocked")
        precondition(tracker.operationID == "synthetic-operation",
                     "Resume must query the accepted operation instead of submitting a new mutation")
        tracker.accept(operationID: tracker.operationID!)
        tracker.observe(state: "pending")
        precondition(tracker.isActive && tracker.isObserving)
        tracker.observe(state: "unrecognized")
        precondition(tracker.isActive, "Unknown status must not unlock a duplicate mutation")
        tracker.observe(state: "succeeded")
        precondition(!tracker.isActive && tracker.operationID == nil)
        tracker.start(label: "Cleanup")
        tracker.accept(operationID: "synthetic-failure")
        tracker.observe(state: "failed")
        precondition(!tracker.isActive, "Only daemon terminal states finish the UI operation")
        var refresh = SnapshotStatusRefreshTracker()
        var visibleCount = 72
        let oldRead = refresh.begin()
        let newRead = refresh.begin()
        let verifiedDate = Date(timeIntervalSince1970: 100)
        if refresh.complete(newRead, succeeded: true, at: verifiedDate) { visibleCount = 0 }
        if refresh.complete(oldRead, succeeded: true) { visibleCount = 72 }
        precondition(visibleCount == 0, "A delayed old response must not restore stale blockers")
        precondition(refresh.lastUpdatedAt == verifiedDate)
        let failedRead = refresh.begin()
        precondition(refresh.complete(failedRead, succeeded: false))
        precondition(refresh.lastUpdatedAt == verifiedDate, "Failed refresh must not claim fresh data")
        let preMutationRead = refresh.begin()
        refresh.invalidate()
        precondition(!refresh.complete(preMutationRead, succeeded: true),
                     "A response from before mutation or tab dismissal must be discarded")
        print("OK: SnapshotMountHelperControlActionTests")
    }
}
