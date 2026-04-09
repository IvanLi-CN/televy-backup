import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func withEnv(_ key: String, _ value: String?, _ body: () -> Void) {
    let oldPtr = getenv(key)
    let oldValue = oldPtr.map { String(cString: $0) }
    if let value {
        setenv(key, value, 1)
    } else {
        unsetenv(key)
    }
    body()
    if let oldValue {
        setenv(key, oldValue, 1)
    } else {
        unsetenv(key)
    }
}

private func withDemoEnv(_ body: () -> Void) {
    withEnv("TELEVYBACKUP_UI_DEMO", "1") {
        withEnv("TELEVYBACKUP_CONFIG_DIR", nil) {
            withEnv("TELEVYBACKUP_DATA_DIR", nil) {
                body()
            }
        }
    }
}

private func runUIDemoSandboxPathTests() {
    withDemoEnv {
        let modelA = AppModel()
        let configA1 = modelA.configTomlPath()
        let configA2 = modelA.configTomlPath()
        let logA1 = modelA.logDirURL()
        let logA2 = modelA.logDirURL()

        expect(configA1 == configA2, "config path should stay stable within one UI demo launch")
        expect(logA1 == logA2, "log path should stay stable within one UI demo launch")
        expect(configA1.path.contains("/TelevyBackup-ui-demo/"), "UI demo fallback should use temp sandbox path")
        expect(!configA1.path.contains("/Library/Application Support/TelevyBackup/config.toml"), "UI demo fallback must not reuse Application Support config")

        let modelB = AppModel()
        let configB = modelB.configTomlPath()
        expect(configB != configA1, "separate UI demo launches should not share one fixed sandbox root")
    }

    print("OK: UIDemoSandboxPathTests")
}

@main
enum UIDemoSandboxPathTestsMain {
    static func main() {
        runUIDemoSandboxPathTests()
    }
}
