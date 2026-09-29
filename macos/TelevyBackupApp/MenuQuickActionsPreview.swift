import SwiftUI

struct MenuQuickActionsPreview: View {
    private let state = TargetPresentation.menuBackupControlState(
        snap: MenuQuickActionsPreview.previewSnapshot,
        backupRequest: nil,
        backupStopRequest: nil,
        lifecycleBusy: false,
        nowMs: 1
    )
    private let browseEntries = SnapshotBrowsePresentation.menuEntries(
        targets: [
            SnapshotBrowseMenuTargetInput(id: "sync", label: "Sync"),
            SnapshotBrowseMenuTargetInput(id: "projects", label: "Projects"),
            SnapshotBrowseMenuTargetInput(id: "codex", label: "Codex"),
        ],
        mountedTargetIDs: ["projects"],
        browsingTargetIDs: [],
        ejectingTargetIDs: []
    )

    var body: some View {
        VStack(spacing: 0) {
            row("Backup", icon: "play.fill", enabled: state == MenuBackupControlState.backupAvailable)
            row("Stop Backup", icon: "stop.fill", enabled: state == MenuBackupControlState.stopAvailable)
            Divider().padding(.vertical, 4)
            submenu("Browse Backups in Finder", icon: "folder", entries: browseEntries, enabled: true)
            submenu(
                "Eject Backup Volume",
                icon: "eject",
                entries: browseEntries.filter(\.showsEject),
                enabled: !browseEntries.filter(\.showsEject).isEmpty
            )
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

    private func submenu(
        _ title: String,
        icon: String,
        entries: [SnapshotBrowseMenuEntry],
        enabled: Bool
    ) -> some View {
        VStack(spacing: 0) {
            HStack(spacing: 10) {
                Image(systemName: icon)
                    .frame(width: 16)
                Text(title)
                Spacer()
                Image(systemName: "chevron.right")
                    .font(.system(size: 10, weight: .semibold))
            }
            .font(.system(size: 13))
            .foregroundStyle(enabled ? .primary : .secondary)
            .padding(.horizontal, 8)
            .frame(height: 27)
            .opacity(enabled ? 1 : 0.48)

            ForEach(entries) { entry in
                HStack(spacing: 8) {
                    Text(entry.title)
                    Spacer()
                    Text(entry.isMounted ? "Mounted" : "Available")
                        .foregroundStyle(.secondary)
                }
                .font(.system(size: 11))
                .padding(.leading, 34)
                .padding(.trailing, 8)
                .frame(height: 22)
                .foregroundStyle(entry.browseEnabled || entry.ejectEnabled ? .primary : .secondary)
            }
        }
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
                activeTask: StatusActiveTask(kind: "backup", directions: ["up"]),
                backupQueue: StatusBackupQueue(activeBatchId: "preview-batch", pendingBatchId: nil)
            ),
        ]
    )
}
