import Combine
import Foundation

@main
enum SnapshotMaintenancePresentationTests {
    static func main() {
        let store = SettingsStore()
        var observed: [SnapshotMountOperationTracker] = []
        let subscription = store.$snapshotMountOperation.sink { observed.append($0) }
        store.snapshotMountOperation.start(label: "Snapshot inventory reconciliation")
        store.snapshotMountOperation.accept(operationID: "synthetic-operation")
        store.snapshotMountOperation.suspend()
        let suspendedCount = observed.count

        // A recreated Settings consumer observes the same session-owned store.
        let reopenedConsumer = store
        precondition(reopenedConsumer.snapshotMountOperation.operationID == "synthetic-operation")
        reopenedConsumer.snapshotMountOperation.accept(operationID: "synthetic-operation")
        precondition(observed.count == suspendedCount + 1, "Resume must publish immediately without waiting for a refresh")
        precondition(observed.last?.isObserving == true)
        precondition(observed.last?.operationID == "synthetic-operation")
        reopenedConsumer.snapshotMountOperation.observe(state: "succeeded")
        precondition(observed.last?.isActive == false, "Terminal state must immediately publish unlocked controls")
        withExtendedLifetime(subscription) {}
        print("OK: SnapshotMaintenancePresentationTests")
    }
}
