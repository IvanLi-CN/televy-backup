import Foundation

enum SnapshotMountHelperControlAction {
    static let reconcileInventoryMethod = "snapshot.mount_helper.reconcile_inventory"

    static func reconcileInventory(using send: @escaping (String) -> Void) -> () -> Void {
        {
            send(reconcileInventoryMethod)
        }
    }
}
