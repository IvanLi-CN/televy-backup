import SwiftUI

struct MenuQuickActionsPreview: View {
    private let state = TargetPresentation.menuBackupControlState(
        snap: MenuQuickActionsPreview.previewSnapshot,
        backupRequest: nil,
        backupStopRequest: nil,
        lifecycleBusy: false,
        nowMs: 1
    )

    var body: some View {
        VStack(spacing: 0) {
            row("Backup", icon: "play.fill", enabled: state == .backupAvailable)
            row("Stop Backup", icon: "stop.fill", enabled: state == .stopAvailable)
            Divider().padding(.vertical, 4)
            row("Main Window", icon: "rectangle.grid.2x2.fill", enabled: true)
            row("Settings", icon: "gearshape", enabled: true)
            Divider().padding(.vertical, 4)
            row("Quit GUI", icon: "rectangle.portrait.and.arrow.right", enabled: true)
            row("Quit Completely", icon: "power", enabled: true)
        }
        .padding(6)
        .frame(width: 280)
        .background(.regularMaterial)
    }

    private func row(_ title: String, icon: String, enabled: Bool) -> some View {
        HStack(spacing: 10) {
            Image(systemName: icon)
                .frame(width: 16)
            Text(title)
            Spacer()
        }
        .font(.system(size: 13))
        .foregroundStyle(enabled ? .primary : .secondary)
        .padding(.horizontal, 8)
        .frame(height: 27)
        .opacity(enabled ? 1 : 0.48)
    }

    private static let previewSnapshot = StatusSnapshot(
        type: "status.snapshot",
        schemaVersion: 1,
        generatedAt: 1,
        source: StatusSource(kind: "daemon", detail: "menu-quick-actions"),
        global: StatusGlobal(
            up: StatusRate(bytesPerSecond: nil),
            down: StatusRate(bytesPerSecond: nil),
            upTotal: StatusCounter(bytes: nil),
            downTotal: StatusCounter(bytes: nil),
            uiUptimeSeconds: nil
        ),
        targets: [
            StatusTarget(
                targetId: "preview-target",
                label: "Preview target",
                sourcePath: "/preview",
                endpointId: "preview-endpoint",
                enabled: true,
                state: "running",
                runningSince: 1,
                up: StatusRate(bytesPerSecond: nil),
                upTotal: StatusCounter(bytes: nil),
                progress: nil,
                lastRun: nil,
                backupQueue: StatusBackupQueue(activeBatchId: "preview-batch", pendingBatchId: nil),
                activeTask: StatusActiveTask(kind: "backup", directions: ["up"])
            ),
        ]
    )
}
