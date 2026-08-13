import Combine
import Foundation

@main
struct StatusStoreTests {
    @MainActor
    static func main() async {
        await idleHeartbeatDoesNotPublish()
        await connectionTransitionsPublishOnce()
        await runningUpdatesAreCoalescedAndFinalIsPreserved()
        await runningPublishCadenceIsAtMostTwoHertz()
        print("StatusStoreTests: PASS")
    }

    @MainActor
    private static func idleHeartbeatDoesNotPublish() async {
        let store = StatusStore(publishInterval: 0.05)
        var publishes = 0
        let token = store.objectWillChange.sink { publishes += 1 }
        store.ingest(snapshot(generatedAt: 1_000, state: "idle"), receivedAt: Date(timeIntervalSince1970: 1))
        store.ingest(snapshot(generatedAt: 2_000, state: "idle"), receivedAt: Date(timeIntervalSince1970: 2))
        precondition(publishes == 1, "equivalent idle heartbeat published \(publishes) times")
        precondition(store.latestReceivedAt == Date(timeIntervalSince1970: 2))
        _ = token
    }

    @MainActor
    private static func connectionTransitionsPublishOnce() async {
        let store = StatusStore()
        var publishes = 0
        let token = store.objectWillChange.sink { publishes += 1 }
        let received = Date(timeIntervalSince1970: 100)
        store.ingest(snapshot(generatedAt: 100_000, state: "idle"), receivedAt: received)
        store.refreshConnectionPhase(now: received.addingTimeInterval(6))
        store.refreshConnectionPhase(now: received.addingTimeInterval(7))
        store.refreshConnectionPhase(now: received.addingTimeInterval(61))
        store.refreshConnectionPhase(now: received.addingTimeInterval(62))
        precondition(publishes == 3, "connection phases published \(publishes) times")
        _ = token
    }

    @MainActor
    private static func runningUpdatesAreCoalescedAndFinalIsPreserved() async {
        let store = StatusStore(publishInterval: 0.05)
        var publishes = 0
        let token = store.objectWillChange.sink { publishes += 1 }
        let start = Date()
        store.ingest(snapshot(generatedAt: 1, state: "running", uploaded: 1), receivedAt: start)
        store.ingest(snapshot(generatedAt: 2, state: "running", uploaded: 2), receivedAt: start.addingTimeInterval(0.01))
        store.ingest(snapshot(generatedAt: 3, state: "running", uploaded: 3), receivedAt: start.addingTimeInterval(0.02))
        try? await Task.sleep(nanoseconds: 80_000_000)
        precondition(publishes == 2, "running burst published \(publishes) times")
        precondition(store.state.snapshot?.targets.first?.progress?.bytesUploaded == 3)
        store.ingest(snapshot(generatedAt: 4, state: "idle", uploaded: 4), receivedAt: Date())
        precondition(store.state.snapshot?.targets.first?.state == "idle", "final idle snapshot was lost")
        _ = token
    }

    @MainActor
    private static func runningPublishCadenceIsAtMostTwoHertz() async {
        final class TestClock {
            var now = Date(timeIntervalSince1970: 1_000)
            var scheduled: [(deadline: Date, action: () -> Void)] = []

            func advance(by interval: TimeInterval) {
                now = now.addingTimeInterval(interval)
                let ready = scheduled.filter { $0.deadline <= now }
                scheduled.removeAll { $0.deadline <= now }
                ready.forEach { $0.action() }
            }
        }
        let clock = TestClock()
        let store = StatusStore(
            now: { clock.now },
            schedule: { delay, action in
                clock.scheduled.append((clock.now.addingTimeInterval(delay), action))
            }
        )
        var publishTimes: [Date] = []
        let token = store.objectWillChange.sink { publishTimes.append(clock.now) }
        for index in 0..<8 {
            store.ingest(
                snapshot(generatedAt: Int64(index + 1), state: "running", uploaded: Int64(index + 1)),
                receivedAt: clock.now
            )
            clock.advance(by: 0.1)
        }
        clock.advance(by: 0.3)
        precondition(publishTimes.count == 3, "expected initial plus two coalesced publishes, got \(publishTimes.count)")
        for pair in zip(publishTimes, publishTimes.dropFirst()) {
            precondition(pair.1.timeIntervalSince(pair.0) >= 0.5, "running publish cadence exceeded 2Hz")
        }

        store.ingest(snapshot(generatedAt: 20, state: "running", uploaded: 20), receivedAt: Date())
        store.ingest(snapshot(generatedAt: 21, state: "idle", uploaded: 21), receivedAt: Date())
        precondition(store.state.snapshot?.targets.first?.state == "idle", "pending running snapshot delayed final idle commit")
        _ = token
    }

    private static func snapshot(generatedAt: Int64, state: String, uploaded: Int64? = nil) -> StatusSnapshot {
        StatusSnapshot(
            type: "status",
            schemaVersion: 1,
            generatedAt: generatedAt,
            source: StatusSource(kind: "daemon", detail: nil),
            global: StatusGlobal(
                up: StatusRate(bytesPerSecond: nil),
                down: StatusRate(bytesPerSecond: nil),
                upTotal: StatusCounter(bytes: nil),
                downTotal: StatusCounter(bytes: nil),
                uiUptimeSeconds: Double(generatedAt)
            ),
            targets: [StatusTarget(
                targetId: "fixture-a",
                label: "Fixture A",
                sourcePath: "/tmp/fixture-a",
                endpointId: "fixture",
                enabled: false,
                state: state,
                runningSince: state == "running" ? generatedAt : nil,
                up: StatusRate(bytesPerSecond: nil),
                upTotal: StatusCounter(bytes: uploaded),
                progress: uploaded.map { value in
                    StatusProgress(
                        phase: "upload",
                        sourceFilesTotal: nil,
                        sourceBytesTotal: nil,
                        filesTotal: nil,
                        filesDone: nil,
                        chunksTotal: nil,
                        chunksDone: nil,
                        bytesRead: nil,
                        bytesUploaded: value,
                        bytesDownloaded: nil,
                        bytesDeduped: nil
                    )
                },
                lastRun: nil
            )]
        )
    }
}
