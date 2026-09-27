import Foundation

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
}
