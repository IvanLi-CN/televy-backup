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
    expect(normal.runtimeStatusUnavailable, "offline status should flag runtime as unavailable")

    let overridden = decode(#"{"configuredLevel":"verbose","effectiveLevel":"custom","effectiveFilter":"info","source":"environment","overriddenBy":"RUST_LOG","pendingLevel":null,"logDirectory":"/tmp/logs","logBytes":null,"daemonAvailable":true}"#)
    expect(overridden.pickerDisabled, "environment override should disable picker")
    expect(overridden.overriddenBy == "RUST_LOG", "override variable should be visible")

    let debugOverride = decode(#"{"configuredLevel":"normal","effectiveLevel":"custom","effectiveFilter":"debug,sqlx=trace","source":"environment","overriddenBy":"TELEVYBACKUP_LOG","pendingLevel":null,"logDirectory":"/tmp/logs","logBytes":42,"daemonAvailable":true}"#)
    expect(debugOverride.debugWarningVisible, "debug-capable override should show warning")

    let debug = decode(#"{"configuredLevel":"debug","effectiveLevel":"debug","effectiveFilter":"debug","source":"local.toml","overriddenBy":null,"pendingLevel":"debug","logDirectory":"/tmp/logs","logBytes":100,"daemonAvailable":true}"#)
    expect(debug.debugWarningVisible, "debug should show persistent warning")
    expect(debug.pendingLevel == .debug, "pending level should decode")

    let retention = decode(#"{"configuredLevel":"normal","effectiveLevel":"normal","effectiveFilter":"warn","source":"local.toml","overriddenBy":null,"pendingLevel":null,"logDirectory":"/tmp/logs","logBytes":100,"managedLogBytes":80,"managedLogCount":2,"retention":{"max_total_gib":17,"max_age_days":45},"retentionPruneEnabled":true,"daemonAvailable":true}"#)
    expect(retention.managedLogBytes == 80, "managed run-log bytes should decode")
    expect(retention.managedLogCount == 2, "managed run-log count should decode")
    expect(retention.retention == CliLogRetention(maxTotalGiB: 17, maxAgeDays: 45), "retention should decode")
    expect(retention.retentionPruneEnabled == true, "retention pruning status should decode")

    for (value, minimum, maximum) in [(1, 1, 100), (17, 1, 100), (100, 1, 100), (7, 7, 365), (45, 7, 365), (365, 7, 365)] {
        let position = LogRetentionControlMapping.sliderPosition(value: value, minimum: minimum, maximum: maximum)
        expect(
            LogRetentionControlMapping.integerValue(position: position, minimum: minimum, maximum: maximum) == value,
            "logarithmic slider mapping should round-trip \(value)"
        )
    }
    expect(LogRetentionControlMapping.capacityTicks == [1, 2, 5, 10, 20, 50, 100], "capacity ticks should remain stable")
    expect(LogRetentionControlMapping.ageTicks == [7, 14, 30, 60, 90, 180, 365], "age ticks should remain stable")
    expect(LogRetentionControlMapping.ageTickLabel(for: 90) == "90 days", "ordinary age ticks should show days")
    expect(LogRetentionControlMapping.ageTickLabel(for: 180) == "6 months", "180-day tick should show six months")
    expect(LogRetentionControlMapping.ageTickLabel(for: 365) == "1 year", "365-day tick should show one year")

    expect(
        !LogRetentionAutoSavePolicy.shouldSave(
            maxTotalGiB: 17,
            maxAgeDays: 45,
            configured: CliLogRetention(maxTotalGiB: 17, maxAgeDays: 45)
        ),
        "unchanged retention should not queue an auto-save"
    )
    expect(
        LogRetentionAutoSavePolicy.shouldSave(
            maxTotalGiB: 20,
            maxAgeDays: 45,
            configured: CliLogRetention(maxTotalGiB: 17, maxAgeDays: 45)
        ),
        "changed retention should queue an auto-save"
    )
    expect(
        !LogRetentionAutoSavePolicy.shouldSave(
            maxTotalGiB: 0,
            maxAgeDays: 45,
            configured: CliLogRetention(maxTotalGiB: 17, maxAgeDays: 45)
        ),
        "invalid retention should never queue an auto-save"
    )
    expect(
        !LogRetentionAutoSavePolicy.shouldSave(maxTotalGiB: 17, maxAgeDays: 45, configured: nil),
        "missing diagnostics status should not queue an auto-save"
    )

    print("OK: DiagnosticsSettingsTests")
}

@main
enum DiagnosticsSettingsTestsMain {
    static func main() {
        runDiagnosticsSettingsTests()
    }
}
