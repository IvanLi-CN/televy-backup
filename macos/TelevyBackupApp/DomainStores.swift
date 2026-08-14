import Combine
import Foundation

final class SettingsStore: ObservableObject {
    @Published var sourcePath = ""
    @Published var label = "manual"
    @Published var chatId = ""
    @Published var botTokenDraft = ""
    @Published var botTokenDraftIsMasked = false
    @Published var mtprotoApiId = ""
    @Published var mtprotoApiHashDraft = ""
    @Published var mtprotoApiHashDraftIsMasked = false
    @Published var scheduleEnabled = false
    @Published var scheduleKind = "hourly"
    @Published var telegramOk = false
    @Published var telegramStatusText = "Telegram Storage • Offline"
    @Published var botTokenPresent = false
    @Published var masterKeyPresent = false
    @Published var mtprotoApiHashPresent = false
    @Published var mtprotoSessionPresent = false
    @Published var secretPresenceKnown = false
    @Published var secretPresenceFetchInFlight = false
    @Published var telegramValidateOk: Bool?
    @Published var telegramValidateText = "Not validated"
    @Published var refreshInFlight = false
}

final class RunHistoryStore: ObservableObject {
    @Published var runs: [RunLogSummary] = []
    @Published var refreshInFlight = false
}

final class TaskPresentationStore: ObservableObject {
    @Published var toastText: String?
    @Published var toastIsError = false
    @Published var shutdownPresentation: AppModel.ShutdownPresentation?
    @Published var isRunning = false
    @Published var phase = "idle"
    @Published var activeTask: AppModel.ActiveTask?
    @Published var backupRequest: BackupRequestPresentation?
    @Published var popoverResizeToken = 0
    @Published var targetRateEstimates: [String: TargetRateEstimate] = [:]
}

final class DiagnosticsStore: ObservableObject {
    @Published var statusActivity: [AppModel.StatusActivityItem] = []
}
