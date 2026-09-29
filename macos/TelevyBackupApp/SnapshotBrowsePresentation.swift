import Foundation

struct SnapshotBrowseMenuTargetInput: Equatable {
    let id: String
    let label: String?
}

struct SnapshotBrowseMenuEntry: Equatable, Identifiable {
    let id: String
    let title: String
    let isMounted: Bool
    let isBrowsing: Bool
    let isEjecting: Bool

    var browseEnabled: Bool {
        !isMounted && !isBrowsing && !isEjecting
    }

    var showsEject: Bool {
        isMounted || isEjecting
    }

    var ejectEnabled: Bool {
        isMounted && !isEjecting
    }
}

enum SnapshotBrowsePresentation {
    static func activityText(isBrowsing: Bool, isEjecting: Bool) -> String? {
        if isBrowsing { return "Browsing…" }
        if isEjecting { return "Ejecting…" }
        return nil
    }

    static func actionText(isBrowsing: Bool, isMounted: Bool, isEjecting: Bool) -> String {
        if let activity = activityText(isBrowsing: isBrowsing, isEjecting: isEjecting) {
            return activity
        }
        return isMounted ? "Eject backup volume" : "Browse backups in Finder"
    }

    static func menuEntries(
        targets: [SnapshotBrowseMenuTargetInput],
        mountedTargetIDs: Set<String>,
        browsingTargetIDs: Set<String>,
        ejectingTargetIDs: Set<String>
    ) -> [SnapshotBrowseMenuEntry] {
        targets.map { target in
            SnapshotBrowseMenuEntry(
                id: target.id,
                title: target.label.flatMap { $0.isEmpty ? nil : $0 } ?? target.id,
                isMounted: mountedTargetIDs.contains(target.id),
                isBrowsing: browsingTargetIDs.contains(target.id),
                isEjecting: ejectingTargetIDs.contains(target.id)
            )
        }
    }
}
