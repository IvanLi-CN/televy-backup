import Foundation

struct MenuBarLocalTask: Equatable {
    var id: String
    var kind: String
    var state: String
}

struct MenuBarPresentation: Equatable {
    var activity: MenuBarActivityState
    var title: String

    static func make(
        snapshot: StatusSnapshot?,
        connectionPhase: StatusConnectionPhase,
        localTask: MenuBarLocalTask?,
        hasLiveFailure: Bool,
        showsTransferRates: Bool
    ) -> Self {
        var directions = Set<String>()
        var hasBackup = false
        var hasRestore = false
        var hasVerify = false

        func addActivity(kind: String, activityDirections: [String]) {
            switch kind {
            case "backup": hasBackup = true
            case "restore": hasRestore = true
            case "verify": hasVerify = true
            case "sync": break
            default: return
            }
            directions.formUnion(activityDirections.filter { $0 == "up" || $0 == "down" })
        }

        if connectionPhase == .fresh, let snapshot {
            for target in snapshot.targets {
                if let activity = target.activeTask {
                    addActivity(kind: activity.kind, activityDirections: activity.directions)
                }
                if target.backupQueue?.activeBatchId != nil || target.backupQueue?.pendingBatchId != nil {
                    addActivity(kind: "backup", activityDirections: ["up"])
                }
            }
        }

        if let localTask, localTask.state == "running" {
            let directions = directionsForTaskKind(localTask.kind)
            addActivity(kind: localTask.kind, activityDirections: directions)
        }

        let activity: MenuBarActivityState
        if hasLiveFailure {
            activity = .failure
        } else if directions.contains("up") && directions.contains("down") {
            activity = .bidirectional
        } else if hasBackup {
            activity = .backup
        } else if hasRestore {
            activity = .restore
        } else if hasVerify {
            activity = .verify
        } else {
            activity = .idle
        }

        let title = rateTitle(
            snapshot: snapshot,
            connectionPhase: connectionPhase,
            directions: directions,
            showsTransferRates: showsTransferRates
        )
        return Self(activity: activity, title: title)
    }

    private static func directionsForTaskKind(_ kind: String) -> [String] {
        switch kind {
        case "backup": ["up"]
        case "restore": ["down"]
        case "verify": []
        case "sync": ["up", "down"]
        default: []
        }
    }

    private static func rateTitle(
        snapshot: StatusSnapshot?,
        connectionPhase: StatusConnectionPhase,
        directions: Set<String>,
        showsTransferRates: Bool
    ) -> String {
        guard showsTransferRates,
              connectionPhase == .fresh,
              let global = snapshot?.global
        else {
            return ""
        }

        var parts: [String] = []
        if directions.contains("up"), let bytes = global.up.bytesPerSecond, bytes >= 0 {
            parts.append("\u{2191} \(formatBytes(bytes))/s")
        }
        if directions.contains("down"), let bytes = global.down.bytesPerSecond, bytes >= 0 {
            parts.append("\u{2193} \(formatBytes(bytes))/s")
        }
        return parts.joined(separator: " ")
    }
}

final class MenuBarFailureLatch {
    static let duration: TimeInterval = 10

    private var observedActiveTargetIds: Set<String> = []
    private var localTaskStates: [String: String] = [:]
    private(set) var failureExpiresAt: Date?

    func observeStatus(
        snapshot: StatusSnapshot?,
        connectionPhase: StatusConnectionPhase,
        now: Date = Date()
    ) {
        guard connectionPhase == .fresh, let snapshot else {
            observedActiveTargetIds.removeAll()
            return
        }

        for target in snapshot.targets {
            let isActive = target.activeTask != nil || target.state == "running"
            if isActive {
                observedActiveTargetIds.insert(target.targetId)
            } else if target.state == "failed" {
                if observedActiveTargetIds.remove(target.targetId) != nil {
                    latchFailure(now: now)
                }
            } else {
                observedActiveTargetIds.remove(target.targetId)
            }
        }
    }

    func observeLocalTask(_ task: MenuBarLocalTask?, now: Date = Date()) {
        guard let task else { return }
        let previous = localTaskStates[task.id]
        localTaskStates[task.id] = task.state
        if task.state == "failed", previous != "failed" {
            latchFailure(now: now)
        }
        if task.state == "succeeded" || task.state == "cancelled" {
            localTaskStates.removeValue(forKey: task.id)
        }
    }

    func isActive(now: Date = Date()) -> Bool {
        guard let failureExpiresAt else { return false }
        guard now < failureExpiresAt else {
            self.failureExpiresAt = nil
            return false
        }
        return true
    }

    private func latchFailure(now: Date) {
        failureExpiresAt = now.addingTimeInterval(Self.duration)
    }
}

enum MenuBarPreferences {
    static let showTransferRatesKey = "showMenuBarTransferRates"

    static func showsTransferRates(defaults: UserDefaults = .standard) -> Bool {
        defaults.bool(forKey: showTransferRatesKey)
    }

    static func setShowsTransferRates(_ shows: Bool, defaults: UserDefaults = .standard) {
        defaults.set(shows, forKey: showTransferRatesKey)
    }
}
