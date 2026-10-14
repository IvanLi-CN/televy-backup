import Foundation

private let executable = "/Applications/TelevyBackup.app/Contents/Library/LoginItems/TelevyBackup Snapshot Access.app/Contents/MacOS/televybackup-snapshot-access"
private let service = "gui/501/com.ivan.televybackup.snapshot-access"
private func job(_ pid: Int32?, program: String = executable) -> SnapshotAccessLaunchAgent.CommandResult {
    .init(status: 0, output: "program = \(program)\n" + (pid.map { "pid = \($0)\n" } ?? ""))
}

private func check(
    initial: SnapshotAccessLaunchAgent.CommandResult,
    current: Bool = false,
    second: SnapshotAccessLaunchAgent.CommandResult? = nil,
    prepare: () throws -> Void = {},
    bootstrapSucceeds: Bool = true,
    expectedKickstart: [String]? = nil,
    expectedFailure: Bool = false
) {
    var commands: [[String]] = []
    var probes = 0
    var prepared = false
    var failed = false
    do {
        try SnapshotAccessLaunchAgent.register(
            service: service, domain: "gui/501", plistPath: "/fixture/agent.plist", executablePath: executable,
            run: { args in
                commands.append(args)
                if args.first == "print" {
                    probes += 1
                    return probes == 1 ? initial : (second ?? initial)
                }
                if args.first == "bootstrap" && !bootstrapSucceeds {
                    return .init(status: 5, output: "Bootstrap failed: 5: Input/output error")
                }
                return .init(status: 0, output: "")
            },
            isCurrent: { _ in current },
            prepareRestart: { prepared = true; try prepare() }
        )
    } catch { failed = true }
    precondition(failed == expectedFailure, "unexpected registration verdict")
    precondition(commands.first(where: { $0.first == "kickstart" }) == expectedKickstart, "unexpected service restart")
    if current || initial.output.contains("/foreign/") { precondition(!prepared) }
}

@main enum SnapshotAccessLaunchAgentTestsMain {
    static func main() throws {
        // Ad-hoc jobs have no parent bundle metadata. Actual runtime identity decides.
        check(initial: job(42), expectedKickstart: ["kickstart", "-k", service])
        check(initial: job(42), current: true)
        check(initial: job(nil), expectedKickstart: ["kickstart", service])
        check(initial: .init(status: 113, output: "Could not find service"))
        // A duplicate bootstrap error 5 is recoverable only after inspecting the job.
        check(initial: .init(status: 113, output: "missing"), second: job(42),
              bootstrapSucceeds: false, expectedKickstart: ["kickstart", "-k", service])
        check(initial: .init(status: 113, output: "missing"), bootstrapSucceeds: false, expectedFailure: true)
        check(initial: job(42, program: "/foreign/access"), expectedFailure: true)
        check(initial: job(42), second: job(43), expectedFailure: true)
        for status in [
            SnapshotAccessRestartStatus(serviceReachable: true, activeLeases: 1, pendingCleanup: 0),
            SnapshotAccessRestartStatus(serviceReachable: true, activeLeases: 0, pendingCleanup: 1),
            SnapshotAccessRestartStatus(serviceReachable: false, activeLeases: 0, pendingCleanup: 0),
            SnapshotAccessRestartStatus(serviceReachable: true, activeLeases: nil, pendingCleanup: 0),
        ] {
            check(initial: job(42), prepare: { try status.requireIdle() }, expectedFailure: true)
        }
        try SnapshotAccessRestartStatus(serviceReachable: true, activeLeases: 0, pendingCleanup: 0).requireIdle()
        print("OK: SnapshotAccessLaunchAgentTests")
    }
}
