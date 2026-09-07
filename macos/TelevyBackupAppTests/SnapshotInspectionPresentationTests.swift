import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func run(kind: String, status: String?, snapshotId: String?) -> RunLogSummary {
    RunLogSummary(
        id: "run",
        runId: "run",
        kind: kind,
        targetId: "target",
        endpointId: "endpoint",
        sourcePath: "/source",
        snapshotId: snapshotId,
        status: status,
        errorCode: nil,
        durationSeconds: nil,
        startedAt: nil,
        finishedAt: nil,
        logURL: URL(fileURLWithPath: "/tmp/run.ndjson"),
        bytesUploaded: nil,
        bytesDeduped: nil,
        bytesWritten: nil,
        bytesChecked: nil,
        filesRestored: nil,
        chunksDownloaded: nil,
        chunksChecked: nil,
        ignoreRuleFiles: nil,
        ignoreInvalidRules: nil
    )
}

private func testSnapshotInspectionEligibility() {
    expect(
        SnapshotInspectionEligibility.forRun(run(kind: "backup", status: "succeeded", snapshotId: "snp_1")) == .inspectable,
        "successful backup with snapshot id should be inspectable"
    )
    expect(
        SnapshotInspectionEligibility.forRun(run(kind: "backup", status: "failed", snapshotId: "snp_1"))
            == .unavailable("File and block data are available only for successful backup runs."),
        "failed backup must not request snapshot files"
    )
    expect(
        SnapshotInspectionEligibility.forRun(run(kind: "backup", status: "succeeded", snapshotId: nil))
            == .unavailable("This run does not have a retained snapshot identifier."),
        "successful run without snapshot id must remain summary-only"
    )
}

private func testTargetSelectionDismissesUnrelatedSnapshotDetail() {
    expect(
        SnapshotRunDetailSelection.shouldKeepDetail(
            runTargetId: "target",
            selectedTargetId: "target",
            unknownTargetId: "__unknown_target__"
        ),
        "snapshot detail should remain visible for its selected target"
    )
    expect(
        !SnapshotRunDetailSelection.shouldKeepDetail(
            runTargetId: "target",
            selectedTargetId: "other-target",
            unknownTargetId: "__unknown_target__"
        ),
        "switching targets should dismiss the previous target snapshot detail"
    )
    expect(
        SnapshotRunDetailSelection.shouldKeepDetail(
            runTargetId: nil,
            selectedTargetId: "__unknown_target__",
            unknownTargetId: "__unknown_target__"
        ),
        "unknown-target snapshot detail should remain visible for the unknown target selection"
    )
}

private func testTreeExpansionSurvivesAsyncReload() {
    let expanded = Set([
        "Ivan",
        "Ivan/code-vibe-monitor",
        "Ivan/code-vibe-monitor/.git",
        "Ivan/code-vibe-monitor/.git/.github",
    ])
    let restored = SnapshotOutlineExpansion.pathsToRestore(
        previouslyExpanded: expanded,
        availablePaths: expanded.union(["Ivan/code-vibe-monitor/.git/.github/workflows"])
    )
    expect(
        restored == expanded,
        "reloading a directory's children must preserve its expanded ancestor chain"
    )
    expect(
        SnapshotOutlineExpansion.pathsToRestore(
            previouslyExpanded: expanded,
            availablePaths: ["Ivan", "Ivan/code-vibe-monitor"]
        ) == ["Ivan", "Ivan/code-vibe-monitor"],
        "paths removed by a refreshed tree must not be restored"
    )
}

private func testOutlinePaginationChoosesNearestRemainingParent() {
    let parentPathByEntry = [
        "Projects": "",
        "Projects/app": "Projects",
        "Projects/app/file.swift": "Projects/app",
    ]
    expect(
        SnapshotOutlinePagination.nextParent(
            afterVisiblePath: "Projects/app/file.swift",
            parentPathByEntry: parentPathByEntry,
            parentsWithMorePages: ["Projects/app", ""]
        ) == "Projects/app",
        "the tree should continue the nearest visible directory before an ancestor page"
    )
    expect(
        SnapshotOutlinePagination.nextParent(
            afterVisiblePath: "Projects/app/file.swift",
            parentPathByEntry: parentPathByEntry,
            parentsWithMorePages: [""]
        ) == "",
        "the tree should fall back to the root page when a visible branch is complete"
    )
}

private func testStoragePaginationPrioritizesExpandedSlices() {
    expect(
        SnapshotStoragePagination.target(
            lastVisibleBlockStorageID: "sto_pack",
            hasMoreStoragePages: true,
            storageBlockIDsWithMorePages: ["sto_pack"]
        ) == .blocks("sto_pack"),
        "scrolling past a partial expanded Pack must request its next slice page first"
    )
    expect(
        SnapshotStoragePagination.target(
            lastVisibleBlockStorageID: "sto_pack",
            hasMoreStoragePages: true,
            storageBlockIDsWithMorePages: []
        ) == .objects,
        "once expanded slices are complete, the Storage object list should continue"
    )
}

private func testBlockRequestEpochRejectsStaleResults() {
    var epoch = SnapshotBlockRequestEpoch()
    let unfilteredRequest = epoch.issue()
    _ = epoch.issue()
    expect(
        !epoch.accepts(unfilteredRequest),
        "a block response from before a filter change must be discarded"
    )

    let filteredRequest = epoch.issue()
    expect(
        epoch.accepts(filteredRequest),
        "the latest block response must remain applicable"
    )
}

private func testStorageRequestEpochRejectsStaleExpansion() {
    var epoch = SnapshotBlockRequestEpoch()
    let packRequest = epoch.issue()
    let directRequest = epoch.issue()
    expect(!epoch.accepts(packRequest), "a storage response from before an expansion must be discarded")
    expect(epoch.accepts(directRequest), "the latest storage expansion response must remain applicable")
}

private func testStoragePreparationDoesNotShowPageLoader() {
    expect(
        !SnapshotStorageLoadingPresentation.showsPageLoader(
            storageLoading: true,
            preparationState: "preparing"
        ),
        "polling a local Storage index must not show the object-page loader"
    )
    expect(
        SnapshotStorageLoadingPresentation.showsPageLoader(
            storageLoading: true,
            preparationState: nil
        ),
        "a ready object-page request should show the object-page loader"
    )
}

@main
enum SnapshotInspectionPresentationTestsMain {
    static func main() {
        testSnapshotInspectionEligibility()
        testTargetSelectionDismissesUnrelatedSnapshotDetail()
        testTreeExpansionSurvivesAsyncReload()
        testOutlinePaginationChoosesNearestRemainingParent()
        testStoragePaginationPrioritizesExpandedSlices()
        testBlockRequestEpochRejectsStaleResults()
        testStorageRequestEpochRejectsStaleExpansion()
        testStoragePreparationDoesNotShowPageLoader()
        print("OK: SnapshotInspectionPresentationTests")
    }
}
