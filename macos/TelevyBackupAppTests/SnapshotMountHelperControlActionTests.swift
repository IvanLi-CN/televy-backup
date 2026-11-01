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
        print("OK: SnapshotMountHelperControlActionTests")
    }
}
