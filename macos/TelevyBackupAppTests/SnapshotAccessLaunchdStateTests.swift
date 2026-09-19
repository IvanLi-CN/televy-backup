import Foundation

private let currentJob = """
gui/501/com.ivan.televybackup.snapshot-access = {
    parent bundle identifier = com.ivan.televybackup
    parent bundle version = 794
    job state = spawn scheduled
}
"""

private let staleFailedJob = """
gui/501/com.ivan.televybackup.snapshot-access = {
    parent bundle identifier = com.ivan.televybackup
    parent bundle version = 705
    last exit code = 78: EX_CONFIG
    job state = spawn failed
}
"""

private let staleBundleJob = """
gui/501/com.ivan.televybackup.snapshot-access = {
    parent bundle identifier = com.ivan.televybackup
    parent bundle version = 705
}
"""

@main
enum SnapshotAccessLaunchdStateTestsMain {
    static func main() {
        precondition(
            !SnapshotAccessLaunchdState.needsRefresh(
                output: currentJob,
                expectedBundleIdentifier: "com.ivan.televybackup",
                expectedBundleVersion: "794"
            )
        )
        precondition(
            SnapshotAccessLaunchdState.needsRefresh(
                output: staleFailedJob,
                expectedBundleIdentifier: "com.ivan.televybackup",
                expectedBundleVersion: "794"
            )
        )
        precondition(
            SnapshotAccessLaunchdState.needsRefresh(
                output: staleBundleJob,
                expectedBundleIdentifier: "com.ivan.televybackup",
                expectedBundleVersion: "794"
            )
        )
        print("OK: SnapshotAccessLaunchdStateTests")
    }
}
