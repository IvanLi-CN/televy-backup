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
