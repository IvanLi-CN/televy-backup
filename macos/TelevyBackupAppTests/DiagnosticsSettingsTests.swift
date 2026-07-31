import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func decode(_ json: String) -> CliDiagnosticsStatus {
    do {
        return try JSONDecoder().decode(CliDiagnosticsStatus.self, from: Data(json.utf8))
    } catch {
        fputs("FAIL: diagnostics decode failed: \(error)\n", stderr)
        exit(1)
    }
}

private func runDiagnosticsSettingsTests() {
    let normal = decode(#"{"configuredLevel":"normal","effectiveLevel":"normal","effectiveFilter":"warn","source":"default","overriddenBy":null,"pendingLevel":null,"logDirectory":"/tmp/logs","logBytes":42,"daemonAvailable":false}"#)
    expect(normal.configuredLevel == .normal, "normal level should decode")
    expect(!normal.pickerDisabled, "normal status should leave picker enabled")
    expect(!normal.debugWarningVisible, "normal status should hide debug warning")

    let overridden = decode(#"{"configuredLevel":"verbose","effectiveLevel":"custom","effectiveFilter":"info","source":"environment","overriddenBy":"RUST_LOG","pendingLevel":null,"logDirectory":"/tmp/logs","logBytes":null,"daemonAvailable":true}"#)
    expect(overridden.pickerDisabled, "environment override should disable picker")
    expect(overridden.overriddenBy == "RUST_LOG", "override variable should be visible")

    let debug = decode(#"{"configuredLevel":"debug","effectiveLevel":"debug","effectiveFilter":"debug","source":"local.toml","overriddenBy":null,"pendingLevel":"debug","logDirectory":"/tmp/logs","logBytes":100,"daemonAvailable":true}"#)
    expect(debug.debugWarningVisible, "debug should show persistent warning")
    expect(debug.pendingLevel == .debug, "pending level should decode")

    print("OK: DiagnosticsSettingsTests")
}

@main
enum DiagnosticsSettingsTestsMain {
    static func main() {
        runDiagnosticsSettingsTests()
    }
}
