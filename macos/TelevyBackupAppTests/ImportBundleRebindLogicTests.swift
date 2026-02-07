import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func expectEqual<T: Equatable>(_ a: T, _ b: T, _ message: String) {
    expect(a == b, "\(message) (got=\(a) expected=\(b))")
}

private func test_selectionAction_samePath_remoteLatestMissing_clearsResolution() {
    let action = RebindApplyGate.selectionAction(
        originalSourcePath: "/Users/ivan/Photos",
        selectedSourcePath: "/Users/ivan/Photos",
        remoteLatestExists: false,
        canClearResolution: true
    )
    expectEqual(action, .clearResolution, "same path without remote latest should clear resolution")
}

private func test_selectionAction_samePath_remoteLatestMissing_needsResolution_rebinds() {
    let action = RebindApplyGate.selectionAction(
        originalSourcePath: "/Users/ivan/Photos",
        selectedSourcePath: "/Users/ivan/Photos",
        remoteLatestExists: false,
        canClearResolution: false
    )
    expectEqual(
        action,
        .rebindAndCompare(newSourcePath: "/Users/ivan/Photos"),
        "same path without remote latest should keep rebind when resolution cannot be cleared"
    )
}

private func test_selectionAction_samePath_remoteLatestExists_rebinds() {
    let action = RebindApplyGate.selectionAction(
        originalSourcePath: "/Users/ivan/Photos",
        selectedSourcePath: "/Users/ivan/Photos",
        remoteLatestExists: true,
        canClearResolution: true
    )
    expectEqual(
        action,
        .rebindAndCompare(newSourcePath: "/Users/ivan/Photos"),
        "same path with remote latest should rebind+compare"
    )
}

private func test_selectionAction_differentPath_always_rebinds() {
    let actionA = RebindApplyGate.selectionAction(
        originalSourcePath: "/Users/ivan/Photos",
        selectedSourcePath: "/Users/ivan/NewPhotos",
        remoteLatestExists: false,
        canClearResolution: true
    )
    expectEqual(
        actionA,
        .rebindAndCompare(newSourcePath: "/Users/ivan/NewPhotos"),
        "different path should rebind+compare even when remote latest is missing"
    )

    let actionB = RebindApplyGate.selectionAction(
        originalSourcePath: "/Users/ivan/Photos",
        selectedSourcePath: "/Users/ivan/NewPhotos",
        remoteLatestExists: true,
        canClearResolution: true
    )
    expectEqual(
        actionB,
        .rebindAndCompare(newSourcePath: "/Users/ivan/NewPhotos"),
        "different path should rebind+compare when remote latest exists"
    )
}

private func test_gate_match_allows_without_decision() {
    let gate = RebindApplyGate.evaluate(
        compareState: .match,
        decision: .undecided,
        isEmptyDirectory: nil
    )
    expectEqual(gate, .allowed, "match should allow apply without asking for a decision")
}

private func test_gate_mismatch_requires_decision() {
    let gate = RebindApplyGate.evaluate(
        compareState: .mismatch,
        decision: .undecided,
        isEmptyDirectory: nil
    )
    expectEqual(gate, .blockedNeedsDecision, "mismatch should require explicit decision")
}

private func test_gate_mismatch_remote_latest_requires_empty_dir() {
    let blocked = RebindApplyGate.evaluate(
        compareState: .mismatch,
        decision: .use_remote_latest,
        isEmptyDirectory: false
    )
    expectEqual(blocked, .blockedNeedsEmptyDirectory, "use_remote_latest should require empty dir")

    let allowed = RebindApplyGate.evaluate(
        compareState: .mismatch,
        decision: .use_remote_latest,
        isEmptyDirectory: true
    )
    expectEqual(allowed, .allowed, "use_remote_latest with empty dir should allow apply")
}

private func test_gate_mismatch_keep_local_and_merge_allow() {
    let keepLocal = RebindApplyGate.evaluate(
        compareState: .mismatch,
        decision: .keep_local,
        isEmptyDirectory: nil
    )
    expectEqual(keepLocal, .allowed, "keep_local should allow apply even when mismatch")

    let merge = RebindApplyGate.evaluate(
        compareState: .mismatch,
        decision: .merge_local_to_remote,
        isEmptyDirectory: nil
    )
    expectEqual(merge, .allowed, "merge_local_to_remote should allow apply even when mismatch")
}

private func test_gate_unknown_checking_error_block() {
    expectEqual(
        RebindApplyGate.evaluate(compareState: .unknown, decision: .undecided, isEmptyDirectory: nil),
        .blockedNeedsCompare,
        "unknown should block apply"
    )
    expectEqual(
        RebindApplyGate.evaluate(compareState: .checking, decision: .undecided, isEmptyDirectory: nil),
        .blockedNeedsCompare,
        "checking should block apply"
    )
    expectEqual(
        RebindApplyGate.evaluate(compareState: .error, decision: .undecided, isEmptyDirectory: nil),
        .blockedCompareError,
        "error should block apply"
    )
}

private func runAllTests() {
    test_selectionAction_samePath_remoteLatestMissing_clearsResolution()
    test_selectionAction_samePath_remoteLatestMissing_needsResolution_rebinds()
    test_selectionAction_samePath_remoteLatestExists_rebinds()
    test_selectionAction_differentPath_always_rebinds()

    test_gate_match_allows_without_decision()
    test_gate_mismatch_requires_decision()
    test_gate_mismatch_remote_latest_requires_empty_dir()
    test_gate_mismatch_keep_local_and_merge_allow()
    test_gate_unknown_checking_error_block()
}

@main
enum ImportBundleRebindLogicTestsMain {
    static func main() {
        runAllTests()
        print("OK: ImportBundleRebindLogicTests")
    }
}
