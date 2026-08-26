import Foundation

struct MenuBarLocalTask: Equatable {
    var id: String
    var kind: String
    var state: String
    var targetId: String? = nil

    static func eventTaskKind(commandArguments: [String]) -> String? {
        guard commandArguments.contains("--events") else { return nil }
        return ["backup", "restore", "verify"].first { commandArguments.contains($0) }
    }
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
                if let activity = target.activeTask, activity.isSupported {
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

    private enum FailureIdentity: Hashable {
        case localTask(String)
        case daemonTargetActivity(targetId: String, generation: Int)
    }

    private var observedActiveTargetIds: Set<String> = []
    private var activeFailureIdentityByTarget: [String: FailureIdentity] = [:]
    private var terminalFailureIdentityByTarget: [String: FailureIdentity] = [:]
    private var terminalLocalTaskIdByTarget: [String: String] = [:]
    private var targetActivityGenerations: [String: Int] = [:]
    private var localTaskStates: [String: String] = [:]
    private var localTaskIdByTarget: [String: String] = [:]
    private var failureExpiryByIdentity: [FailureIdentity: Date] = [:]
    private(set) var failureExpiresAt: Date?

    func observeStatus(
        snapshot: StatusSnapshot?,
        connectionPhase: StatusConnectionPhase,
        now: Date = Date()
    ) {
        guard connectionPhase == .fresh, let snapshot else {
            resetStatusSession()
            return
        }

        for target in snapshot.targets {
            let isActive = target.activeTask?.isSupported == true
            if isActive {
                if observedActiveTargetIds.insert(target.targetId).inserted {
                    beginStatusActivity(for: target.targetId)
                }
            } else if target.state == "failed" {
                if observedActiveTargetIds.remove(target.targetId) != nil {
                    let identity = activeFailureIdentityByTarget.removeValue(forKey: target.targetId)
                        ?? nextDaemonIdentity(for: target.targetId)
                    terminalFailureIdentityByTarget[target.targetId] = identity
                    if case let .localTask(taskId) = identity {
                        terminalLocalTaskIdByTarget[target.targetId] = taskId
                    } else {
                        terminalLocalTaskIdByTarget.removeValue(forKey: target.targetId)
                    }
                    latchFailure(identity, now: now)
                }
            } else {
                observedActiveTargetIds.remove(target.targetId)
                activeFailureIdentityByTarget.removeValue(forKey: target.targetId)
                terminalFailureIdentityByTarget.removeValue(forKey: target.targetId)
                terminalLocalTaskIdByTarget.removeValue(forKey: target.targetId)
            }
        }
    }

    func observeLocalTask(_ task: MenuBarLocalTask?, now: Date = Date()) {
        guard let task else {
            localTaskStates = localTaskStates.filter { $0.value == "running" }
            localTaskIdByTarget = localTaskIdByTarget.filter { localTaskStates[$0.value] == "running" }
            return
        }
        let previous = localTaskStates[task.id]
        localTaskStates[task.id] = task.state
        let targetId = task.targetId.flatMap { $0.isEmpty ? nil : $0 }

        if task.state == "running", let targetId {
            localTaskIdByTarget[targetId] = task.id
            terminalFailureIdentityByTarget.removeValue(forKey: targetId)
            terminalLocalTaskIdByTarget.removeValue(forKey: targetId)
            if observedActiveTargetIds.contains(targetId) {
                activeFailureIdentityByTarget[targetId] = .localTask(task.id)
            }
        }

        if task.state == "failed", previous != "failed" {
            latchFailure(failureIdentity(for: task.id, targetId: targetId, previousState: previous), now: now)
        }
        if task.state == "succeeded" || task.state == "cancelled" {
            localTaskStates.removeValue(forKey: task.id)
            if let targetId, localTaskIdByTarget[targetId] == task.id {
                localTaskIdByTarget.removeValue(forKey: targetId)
            }
        }
    }

    func resetStatusSession() {
        observedActiveTargetIds.removeAll()
        activeFailureIdentityByTarget.removeAll()
        terminalFailureIdentityByTarget.removeAll()
        terminalLocalTaskIdByTarget.removeAll()
        failureExpiryByIdentity = failureExpiryByIdentity.filter { identity, _ in
            if case .localTask = identity {
                return true
            }
            return false
        }
        failureExpiresAt = failureExpiryByIdentity.values.min()
    }

    static func requiresStatusSessionReset(
        previousIngressAt: Date?,
        now: Date,
        maximumGap: TimeInterval
    ) -> Bool {
        guard let previousIngressAt else { return false }
        return now.timeIntervalSince(previousIngressAt) > maximumGap
    }

    func isActive(now: Date = Date()) -> Bool {
        pruneExpiredFailures(now: now)
        return failureExpiresAt != nil
    }

    private func beginStatusActivity(for targetId: String) {
        let identity: FailureIdentity
        if let taskId = localTaskIdByTarget[targetId], localTaskStates[taskId] == "running" {
            identity = .localTask(taskId)
        } else {
            if let taskId = localTaskIdByTarget[targetId], localTaskStates[taskId] != "running" {
                localTaskIdByTarget.removeValue(forKey: targetId)
            }
            identity = nextDaemonIdentity(for: targetId)
        }
        activeFailureIdentityByTarget[targetId] = identity
        terminalFailureIdentityByTarget.removeValue(forKey: targetId)
        terminalLocalTaskIdByTarget.removeValue(forKey: targetId)
    }

    private func nextDaemonIdentity(for targetId: String) -> FailureIdentity {
        let generation = (targetActivityGenerations[targetId] ?? 0) + 1
        targetActivityGenerations[targetId] = generation
        return .daemonTargetActivity(targetId: targetId, generation: generation)
    }

    private func failureIdentity(
        for taskId: String,
        targetId: String?,
        previousState: String?
    ) -> FailureIdentity {
        guard let targetId else { return .localTask(taskId) }

        if previousState == "running", observedActiveTargetIds.contains(targetId) {
            let identity = FailureIdentity.localTask(taskId)
            activeFailureIdentityByTarget[targetId] = identity
            return identity
        }
        if terminalLocalTaskIdByTarget[targetId] == taskId,
           let identity = terminalFailureIdentityByTarget[targetId]
        {
            return identity
        }
        return .localTask(taskId)
    }

    private func latchFailure(_ identity: FailureIdentity, now: Date) {
        pruneExpiredFailures(now: now)
        guard failureExpiryByIdentity[identity] == nil else { return }
        failureExpiryByIdentity[identity] = now.addingTimeInterval(Self.duration)
        failureExpiresAt = failureExpiryByIdentity.values.min()
    }

    private func pruneExpiredFailures(now: Date) {
        failureExpiryByIdentity = failureExpiryByIdentity.filter { $0.value > now }
        failureExpiresAt = failureExpiryByIdentity.values.min()
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
