import SwiftUI

/// Shared by Settings and the synthetic CI preview; never installs during status refresh.
struct SnapshotMountHelperStatusView: View {
    let readiness: SnapshotMountHelperReadiness
    let path: String?
    let version: String?
    let error: String?
    let activeMounts: UInt32?
    let pendingCleanup: UInt32?
    let pendingCleanupCandidates: UInt32?
    let liveMounts: UInt32?
    let recoverableStaleMounts: UInt32?
    let unknownBlockingMounts: UInt32?
    let inventoryDriftMounts: UInt32?
    let busy: Bool
    let install: () -> Void
    let recover: () -> Void
    let retryCleanup: () -> Void
    let reconcileInventory: (String) -> Void
    let repairLegacy: () -> Void

    init(
        readiness: SnapshotMountHelperReadiness,
        path: String?,
        version: String?,
        error: String?,
        activeMounts: UInt32?,
        pendingCleanup: UInt32?,
        pendingCleanupCandidates: UInt32?,
        liveMounts: UInt32?,
        recoverableStaleMounts: UInt32?,
        unknownBlockingMounts: UInt32?,
        inventoryDriftMounts: UInt32?,
        busy: Bool,
        install: @escaping () -> Void,
        recover: @escaping () -> Void,
        retryCleanup: @escaping () -> Void,
        reconcileInventory: @escaping (String) -> Void,
        repairLegacy: @escaping () -> Void = {}
    ) {
        self.readiness = readiness
        self.path = path
        self.version = version
        self.error = error
        self.activeMounts = activeMounts
        self.pendingCleanup = pendingCleanup
        self.pendingCleanupCandidates = pendingCleanupCandidates
        self.liveMounts = liveMounts
        self.recoverableStaleMounts = recoverableStaleMounts
        self.unknownBlockingMounts = unknownBlockingMounts
        self.inventoryDriftMounts = inventoryDriftMounts
        self.busy = busy
        self.install = install
        self.recover = recover
        self.retryCleanup = retryCleanup
        self.reconcileInventory = reconcileInventory
        self.repairLegacy = repairLegacy
    }

    var body: some View {
        let updateBlockReason = !readiness.isReady
            && !readiness.isLegacyAggregateStatus
            && !readiness.isLegacyPendingCleanupStatus
            && !readiness.isStaleCurrentPartialClassification
            ? SnapshotMountHelperReadiness.updateBlockReason(
                activeMounts: activeMounts,
                pendingCleanup: pendingCleanup
            )
            : nil
        let statusTitle = !readiness.isOperational && !readiness.isReady && updateBlockReason != nil
            && !readiness.isPendingCleanupUpgradeSafe
            ? "Snapshot mount helper update waiting"
            : readiness.title
        VStack(alignment: .leading, spacing: 6) {
            Label(
                statusTitle,
                systemImage: readiness.isOperational ? "checkmark.shield.fill" : "exclamationmark.shield.fill"
            )
                .foregroundStyle(readiness.isOperational ? .green : .orange)
            if let path {
                Text(path)
                    .font(.system(size: 11, design: .monospaced))
                    .textSelection(.enabled)
            }
            if let version {
                Text("Version \(version)").font(.system(size: 11)).foregroundStyle(.secondary)
            }
            if let liveMounts, liveMounts > 0 {
                Text("\(liveMounts) active snapshot mount(s) are still running. Finish the backup before updating the helper.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            }
            if let pendingCleanup, pendingCleanup > 0 {
                Text("\(pendingCleanup) snapshot cleanup record(s) are pending. Strict mode remains blocked until cleanup is complete.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                if readiness.canRetryPendingCleanup {
                    Button("Retry snapshot cleanup", action: retryCleanup)
                        .buttonStyle(.bordered)
                        .disabled(busy)
                }
            }
            if let recoverableStaleMounts, recoverableStaleMounts > 0 {
                Text("\(recoverableStaleMounts) stale snapshot mount record(s) can be safely recovered.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Recover stale mount records", action: recover)
                    .buttonStyle(.bordered)
                    .disabled(busy || !readiness.canRecoverStaleMounts)
            }
            if let unknownBlockingMounts, unknownBlockingMounts > 0 {
                Text("\(unknownBlockingMounts) legacy snapshot record(s) need an explicit safe repair. The repair checks the owning user, age, managed mount root, device identity, and snapshot inventory before reusing Release.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                Button("Repair legacy snapshot records", action: repairLegacy)
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("snapshot-mount-helper-repair-legacy")
                    .disabled(busy || !readiness.canRepairLegacyRecords)
            }
            if let inventoryDriftMounts, inventoryDriftMounts > 0 {
                Text("\(inventoryDriftMounts) released snapshot inventory record(s) changed outside the recorded baseline. Reconciliation updates only the journal baseline after identity checks; it does not delete snapshots.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                Button(
                    "Reconcile snapshot inventory",
                    action: SnapshotMountHelperControlAction.reconcileInventory(using: reconcileInventory)
                )
                    .buttonStyle(.bordered)
                    .accessibilityIdentifier("snapshot-mount-helper-reconcile-inventory")
                    .disabled(busy || !readiness.canReconcileInventory)
            }
            if !readiness.classificationKnown && readiness.isReady {
                Text("The installed helper did not report a complete mount status. Refresh diagnostics before changing it.")
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
            } else if let updateBlockReason {
                Text(updateBlockReason)
                    .font(.system(size: 11))
                    .foregroundStyle(.orange)
                    .fixedSize(horizontal: false, vertical: true)
                if let explanation = readiness.updateExplanation {
                    Text(explanation)
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                        .fixedSize(horizontal: false, vertical: true)
                }
            } else if let explanation = readiness.updateExplanation {
                Text(explanation).font(.system(size: 11)).foregroundStyle(.secondary)
            } else if let error, !readiness.isReady {
                Text(error).font(.system(size: 11)).foregroundStyle(.secondary).lineLimit(2)
            }
            if let actionTitle = readiness.actionTitle {
                Button(actionTitle, action: install)
                    .buttonStyle(.borderedProminent)
                    .disabled(busy || !readiness.canUpdate)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
