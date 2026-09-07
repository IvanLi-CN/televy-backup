import Combine
import Foundation

enum MainWindowDestination: Equatable {
    case target(targetId: String)
    case completedRun(targetId: String, runId: String)
    case activeRun(targetId: String, taskId: String)

    var targetId: String {
        switch self {
        case let .target(targetId), let .completedRun(targetId, _), let .activeRun(targetId, _):
            targetId
        }
    }
}

struct MainWindowNavigationRequest: Equatable, Identifiable {
    let revision: Int
    let destination: MainWindowDestination

    var id: Int { revision }
}

final class MainWindowNavigationStore: ObservableObject {
    @Published private(set) var request: MainWindowNavigationRequest?
    private var nextRevision = 0

    func submit(_ destination: MainWindowDestination) {
        nextRevision &+= 1
        request = MainWindowNavigationRequest(revision: nextRevision, destination: destination)
    }
}

enum MainWindowNavigationResolver {
    static func exactRun(
        targetId: String,
        runId: String,
        runs: [RunLogSummary]
    ) -> RunLogSummary? {
        runs.first { $0.targetId == targetId && $0.runId == runId }
    }
}
