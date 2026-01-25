import Foundation

struct StatusRate: Codable {
    var bytesPerSecond: Int64?
}

struct StatusCounter: Codable {
    var bytes: Int64?
}

struct StatusProgress: Codable {
    var phase: String
    var filesTotal: Int64?
    var filesDone: Int64?
    var chunksTotal: Int64?
    var chunksDone: Int64?
    var bytesRead: Int64?
    var bytesUploaded: Int64?
    var bytesDeduped: Int64?
}

struct StatusTargetRunSummary: Codable {
    var finishedAt: String?
    var durationSeconds: Double?
    var status: String?
    var errorCode: String?
    var bytesUploaded: Int64?
    var bytesDeduped: Int64?
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

