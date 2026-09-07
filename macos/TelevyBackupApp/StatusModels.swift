import Foundation

struct StatusRate: Codable {
    var bytesPerSecond: Int64?
}

struct StatusCounter: Codable {
    var bytes: Int64?
}

struct StatusProgress: Codable {
    var phase: String
    var sourceFilesTotal: Int64?
    var sourceBytesTotal: Int64?
    var sourceBytesNeedUploadTotal: Int64? = nil
    var filesTotal: Int64?
    var filesDone: Int64?
    var chunksTotal: Int64?
    var chunksDone: Int64?
    var bytesRead: Int64?
    var uploadBytesTotal: Int64? = nil
    var bytesUploadedConfirmed: Int64? = nil
    var bytesUploadedSource: Int64? = nil
    var bytesUploaded: Int64?
    var bytesDownloaded: Int64?
    var bytesDeduped: Int64?
}

struct StatusTargetRunSummary: Codable {
    var runId: String? = nil
    var kind: String? = nil
    var snapshotId: String? = nil
    var finishedAt: String?
    var durationSeconds: Double?
    var status: String?
    var errorCode: String?
    var filesIndexed: Int64?
    var bytesUploaded: Int64?
    var bytesDeduped: Int64?
}

struct StatusBackupQueue: Codable, Equatable {
    var activeBatchId: String?
    var pendingBatchId: String?
}

struct StatusActiveTask: Codable, Equatable {
    private enum CodingKeys: String, CodingKey {
        case taskId
        case kind
        case directions
    }

    var taskId: String?
    var kind: String
    var directions: [String]

    init(taskId: String? = nil, kind: String, directions: [String]) {
        self.taskId = taskId
        self.kind = kind
        self.directions = directions
    }

    init(from decoder: Decoder) throws {
        guard let container = try? decoder.container(keyedBy: CodingKeys.self) else {
            taskId = nil
            kind = ""
            directions = []
            return
        }
        taskId = try? container.decode(String.self, forKey: .taskId)
        kind = (try? container.decode(String.self, forKey: .kind)) ?? ""
        directions = (try? container.decode([String].self, forKey: .directions)) ?? []
    }

    var isSupported: Bool {
        switch (kind, directions) {
        case ("backup", ["up"]), ("restore", ["down"]), ("verify", []), ("sync", ["up", "down"]):
            true
        default:
            false
        }
    }
}

struct StatusSource: Codable {
    var kind: String
    var detail: String?
}

struct StatusGlobal: Codable {
    var up: StatusRate
    var down: StatusRate
    var upTotal: StatusCounter
    var downTotal: StatusCounter
    var uiUptimeSeconds: Double?
}

struct StatusTarget: Codable, Identifiable {
    var targetId: String
    var label: String?
    var sourcePath: String
    var endpointId: String
    var enabled: Bool
    var state: String
    var runningSince: Int64?
    var up: StatusRate
    var upTotal: StatusCounter
    var progress: StatusProgress?
    var lastRun: StatusTargetRunSummary?
    var activeTask: StatusActiveTask? = nil
    var backupQueue: StatusBackupQueue? = nil

    var id: String { targetId }
}

struct StatusSnapshot: Codable {
    var type: String
    var schemaVersion: Int
    var generatedAt: Int64
    var source: StatusSource
    var global: StatusGlobal
    var targets: [StatusTarget]
}
