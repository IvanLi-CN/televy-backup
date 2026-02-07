import Foundation

// MARK: - Import bundle rebind decision logic
//
// This file intentionally stays free of SwiftUI/AppKit so we can compile and run
// small Swift unit tests without building the full macOS app.

enum RebindCompareState: String {
    case unknown
    case checking
    case match
    case mismatch
    case remote_missing
    case error
}

enum RebindDataDecision: String, CaseIterable, Identifiable {
    case undecided
    case use_remote_latest
    case keep_local
    case merge_local_to_remote

    var id: String { rawValue }

    var title: String {
        switch self {
        case .undecided: return "Choose…"
        case .use_remote_latest: return "Use remote latest"
        case .keep_local: return "Keep local folder"
        case .merge_local_to_remote: return "Merge (backup local to remote)"
        }
    }

    var detail: String {
        switch self {
        case .undecided:
            return "Pick which side should be treated as the source of truth."
        case .use_remote_latest:
            return "Recommended for restore. The folder must be empty."
        case .keep_local:
            return "Local folder becomes the source of truth; next backup may overwrite remote latest."
        case .merge_local_to_remote:
            return "Runs a backup after import to update remote latest from the local folder."
        }
    }
}

enum RebindFolderSelectionAction: Equatable {
    case clearResolution
    case rebindAndCompare(newSourcePath: String)
}

enum RebindApplyGateResult: Equatable {
    case allowed
    case blockedNeedsCompare
    case blockedCompareError
    case blockedNeedsDecision
    case blockedNeedsEmptyDirectory
}

enum RebindApplyGate {
    static func selectionAction(
        originalSourcePath: String?,
        selectedSourcePath: String,
        remoteLatestExists: Bool,
        canClearResolution: Bool
    ) -> RebindFolderSelectionAction {
        // If the user re-selects the original path:
        // - remote latest exists: treat it as a rebind so we can compare and potentially prompt.
        // - remote latest missing: it is a no-op, so clear the rebind resolution (back to defaults).
        if canClearResolution,
           let orig = originalSourcePath,
           orig == selectedSourcePath,
           !remoteLatestExists
        {
            return .clearResolution
        }
        return .rebindAndCompare(newSourcePath: selectedSourcePath)
    }

    static func evaluate(
        compareState: RebindCompareState,
        decision: RebindDataDecision,
        isEmptyDirectory: Bool?
    ) -> RebindApplyGateResult {
        switch compareState {
        case .unknown, .checking:
            return .blockedNeedsCompare
        case .error:
            return .blockedCompareError
        case .match, .remote_missing:
            return .allowed
        case .mismatch:
            if decision == .undecided {
                return .blockedNeedsDecision
            }
            if decision == .use_remote_latest {
                return isEmptyDirectory == true ? .allowed : .blockedNeedsEmptyDirectory
            }
            return .allowed
        }
    }
}
