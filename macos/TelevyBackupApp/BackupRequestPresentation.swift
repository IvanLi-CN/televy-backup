import Foundation

enum BackupRequestPhase: Equatable {
    case starting
    case awaitingDaemonSnapshot
}

struct BackupRequestPresentation: Equatable {
    var targetIds: Set<String>
    var batchId: String?
    var phase: BackupRequestPhase
    var startedAt: Date

    func includes(targetId: String) -> Bool {
        targetIds.contains(targetId)
    }

    func isObserved(in snapshot: StatusSnapshot?) -> Bool {
        guard let batchId, let snapshot else { return false }
        return snapshot.targets.contains {
            $0.backupQueue?.activeBatchId == batchId || $0.backupQueue?.pendingBatchId == batchId
        }
    }
}

enum BackupRequestButtonState: Equatable {
    case idle
    case starting
    case enqueueNext
    case queued

    var isDisabled: Bool {
        switch self {
        case .idle, .enqueueNext: return false
        case .starting, .queued: return true
        }
    }

    var accessibilityLabel: String {
        switch self {
        case .idle: return "Start backup"
        case .starting: return "Starting backup"
        case .enqueueNext: return "Queue another backup"
        case .queued: return "Backup already queued"
        }
    }
}
