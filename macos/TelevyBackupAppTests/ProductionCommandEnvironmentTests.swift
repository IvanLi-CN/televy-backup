import Foundation

@discardableResult
private func expectProductionEnvironment(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

func runProductionCommandEnvironmentTests() {
    let model = AppModel()
    model.testingUsesProductionSnapshotAccessConfiguration = true
    model.testingIsProductionAppVariant = true

    let environment = model.testingTelevybackupToolEnv()

    expectProductionEnvironment(
        environment["TELEVYBACKUP_SNAPSHOT_ACCESS_MANAGER"] == "launchctl-embedded"
            || environment["TELEVYBACKUP_SNAPSHOT_ACCESS_MANAGER"] == "smappservice",
        "production command environment should classify the app signature without recursing"
    )
    print("OK: ProductionCommandEnvironmentTests")
}
