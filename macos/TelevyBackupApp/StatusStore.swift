import Combine
import Foundation

enum StatusFreshness {
    static let staleMs: Int64 = 5_000
    static let disconnectedMs: Int64 = 60_000
    static let toastMaxAgeSeconds: Int = 15
}

enum StatusConnectionPhase: Int, Equatable {
    case fresh
    case stale
    case disconnected
}

final class StatusStore: ObservableObject {
    var onPublish: ((StatusSnapshot) -> Void)?
    struct ViewState {
        var snapshot: StatusSnapshot?
        var receivedAt: Date?
        var connectionPhase: StatusConnectionPhase
    }

    @Published private(set) var state = ViewState(
        snapshot: nil,
        receivedAt: nil,
        connectionPhase: .disconnected
    )

    private(set) var latestSnapshot: StatusSnapshot?
    private(set) var latestReceivedAt: Date?
    private var publishedFingerprint: Data?
    private var lastPublishedAt: Date?
    private var pendingRunningSnapshot: StatusSnapshot?
    private var pendingRunningReceivedAt: Date?
    private var pendingWorkItem: DispatchWorkItem?
    private let publishInterval: TimeInterval

    init(publishInterval: TimeInterval = 0.5) {
        self.publishInterval = publishInterval
    }

    deinit {
        pendingWorkItem?.cancel()
    }

    var snapshot: StatusSnapshot? {
        guard var published = state.snapshot else { return nil }
        if let latestSnapshot {
            published.generatedAt = latestSnapshot.generatedAt
            published.global.uiUptimeSeconds = latestSnapshot.global.uiUptimeSeconds
        }
        return published
    }
    var receivedAt: Date? { latestReceivedAt ?? state.receivedAt }
    var connectionPhase: StatusConnectionPhase { state.connectionPhase }

    @discardableResult
    func ingest(_ snapshot: StatusSnapshot, receivedAt: Date = Date()) -> Bool {
        latestSnapshot = snapshot
        latestReceivedAt = receivedAt

        let fingerprint = Self.semanticFingerprint(snapshot)
        let semanticChange = fingerprint != publishedFingerprint
        let running = snapshot.targets.contains { $0.state == "running" }

        guard running else {
            pendingWorkItem?.cancel()
            pendingWorkItem = nil
            pendingRunningSnapshot = nil
            pendingRunningReceivedAt = nil
            let shouldPublish = semanticChange || state.snapshot == nil || state.connectionPhase != .fresh
            if shouldPublish {
                publish(snapshot, receivedAt: receivedAt, fingerprint: fingerprint, phase: .fresh)
            }
            return shouldPublish
        }

        guard semanticChange else {
            let shouldPublish = state.connectionPhase != .fresh
            if shouldPublish {
                publish(snapshot, receivedAt: receivedAt, fingerprint: fingerprint, phase: .fresh)
            }
            return shouldPublish
        }

        let elapsed = Date().timeIntervalSince(lastPublishedAt ?? .distantPast)
        if elapsed >= publishInterval {
            publish(snapshot, receivedAt: receivedAt, fingerprint: fingerprint, phase: .fresh)
            return true
        }

        pendingRunningSnapshot = snapshot
        pendingRunningReceivedAt = receivedAt
        guard pendingWorkItem == nil else { return false }
        let work = DispatchWorkItem { [weak self] in
            guard let self,
                  let pending = self.pendingRunningSnapshot,
                  let pendingAt = self.pendingRunningReceivedAt
            else { return }
            self.pendingWorkItem = nil
            self.pendingRunningSnapshot = nil
            self.pendingRunningReceivedAt = nil
            self.publish(
                pending,
                receivedAt: pendingAt,
                fingerprint: Self.semanticFingerprint(pending),
                phase: .fresh
            )
        }
        pendingWorkItem = work
        DispatchQueue.main.asyncAfter(deadline: .now() + max(0, publishInterval - elapsed), execute: work)
        return false
    }

    func refreshConnectionPhase(now: Date = Date()) {
        let phase: StatusConnectionPhase
        if let latestReceivedAt {
            let ageMs = Int64(max(0, now.timeIntervalSince(latestReceivedAt)) * 1000)
            phase = ageMs > StatusFreshness.disconnectedMs
                ? .disconnected
                : (ageMs > StatusFreshness.staleMs ? .stale : .fresh)
        } else {
            phase = .disconnected
        }
        guard phase != state.connectionPhase else { return }
        state = ViewState(snapshot: state.snapshot, receivedAt: latestReceivedAt, connectionPhase: phase)
    }

    private func publish(
        _ snapshot: StatusSnapshot,
        receivedAt: Date,
        fingerprint: Data,
        phase: StatusConnectionPhase
    ) {
        publishedFingerprint = fingerprint
        lastPublishedAt = Date()
        state = ViewState(snapshot: snapshot, receivedAt: receivedAt, connectionPhase: phase)
        onPublish?(snapshot)
    }

    private static func semanticFingerprint(_ snapshot: StatusSnapshot) -> Data {
        guard let encoded = try? JSONEncoder().encode(snapshot),
              var object = try? JSONSerialization.jsonObject(with: encoded) as? [String: Any]
        else { return Data() }
        object.removeValue(forKey: "generatedAt")
        if var global = object["global"] as? [String: Any] {
            global.removeValue(forKey: "uiUptimeSeconds")
            object["global"] = global
        }
        return (try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys])) ?? Data()
    }
}
