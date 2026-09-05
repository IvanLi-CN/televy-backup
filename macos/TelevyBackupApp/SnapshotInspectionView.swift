import AppKit
import Combine
import Darwin
import Foundation
import SwiftUI

private struct SnapshotInspectionSummary: Decodable {
    struct Snapshot: Decodable {
        let snapshotId: String
        let createdAt: String
        let sourcePath: String
        let label: String
        let baseSnapshotId: String?
    }

    struct Availability: Decodable {
        let state: String
        let reason: String?
    }

    struct FileCounts: Decodable {
        let entries: UInt64
        let regularFiles: UInt64
        let directories: UInt64
        let symlinks: UInt64
        let bytes: UInt64
    }

    struct ChangeCounts: Decodable {
        let state: String
        let added: UInt64
        let deleted: UInt64
        let changed: UInt64
    }

    struct BlockCounts: Decodable {
        let distinct: UInt64
        let bytes: UInt64
    }

    let snapshot: Snapshot
    let availability: Availability
    let files: FileCounts
    let changes: ChangeCounts
    let blocks: BlockCounts
}

private struct SnapshotFileMetadata: Decodable {
    let kind: String
    let size: UInt64
    let mtimeMs: Int64
    let mode: Int64
}

private struct SnapshotDescendantChanges: Decodable {
    let added: UInt64
    let deleted: UInt64
    let changed: UInt64
}

private struct SnapshotFileEntry: Decodable, Identifiable {
    let path: String
    let name: String
    let kind: String
    let change: String
    let isAncestorContext: Bool
    let size: UInt64
    let mtimeMs: Int64
    let mode: Int64
    let baseline: SnapshotFileMetadata?
    let descendantChanges: SnapshotDescendantChanges?

    var id: String { path }
}

private struct SnapshotFilePage: Decodable {
    let entries: [SnapshotFileEntry]
    let nextCursor: String?
}

private struct SnapshotBlockEntry: Decodable, Identifiable {
    let hash: String
    let size: UInt64
    let changedFiles: UInt64
    let referencingFiles: UInt64

    var id: String { hash }
}

private struct SnapshotBlockPage: Decodable {
    let entries: [SnapshotBlockEntry]
    let nextCursor: String?
}

private struct SnapshotStorageEntry: Decodable, Identifiable {
    let storageId: String
    let kind: String
    let documentBytes: UInt64?
    let recordedAt: String?
    let referencedBlocks: UInt64
    let logicalBytes: UInt64

    var id: String { storageId }
    var shortId: String {
        guard storageId.count > 18 else { return storageId }
        return "\(storageId.prefix(10))...\(storageId.suffix(6))"
    }
}

private struct SnapshotStoragePage: Decodable {
    let entries: [SnapshotStorageEntry]
    let nextCursor: String?
}

private struct SnapshotStorageBlockEntry: Decodable, Identifiable {
    let hash: String
    let size: UInt64
    let offset: UInt64
    let length: UInt64

    var id: String { "\(hash):\(offset):\(length)" }
}

private struct SnapshotStorageBlocksPage: Decodable {
    let entries: [SnapshotStorageBlockEntry]
    let nextCursor: String?
}

struct SnapshotBlockRequestEpoch {
    private(set) var value = 0

    mutating func issue() -> Int {
        value += 1
        return value
    }

    func accepts(_ token: Int) -> Bool {
        token == value
    }
}

enum SnapshotInspectionPresentation: String, CaseIterable, Identifiable {
    case tree = "Tree"
    case list = "List"

    var id: String { rawValue }
    var commandValue: String { self == .tree ? "tree" : "list" }
}

enum SnapshotInspectionEligibility: Equatable {
    case inspectable
    case unavailable(String)

    static func forRun(_ run: RunLogSummary) -> SnapshotInspectionEligibility {
        guard run.kind == "backup", run.status == "succeeded" else {
            return .unavailable("File and block data are available only for successful backup runs.")
        }
        guard run.snapshotId?.isEmpty == false else {
            return .unavailable("This run does not have a retained snapshot identifier.")
        }
        return .inspectable
    }
}

private final class SnapshotInspectionStore: ObservableObject {
    @Published private(set) var summary: SnapshotInspectionSummary?
    @Published private(set) var summaryLoading = false
    @Published private(set) var issue: String?
    @Published private(set) var issueRetryable = false
    @Published private(set) var listEntries: [SnapshotFileEntry] = []
    @Published private(set) var treeEntries: [String: [SnapshotFileEntry]] = [:]
    @Published private(set) var blocks: [SnapshotBlockEntry] = []
    @Published private(set) var storageEntries: [SnapshotStorageEntry] = []
    @Published private(set) var storageBlocks: [String: [SnapshotStorageBlockEntry]] = [:]
    @Published private(set) var filesLoading = false
    @Published private(set) var blocksLoading = false
    @Published private(set) var storageLoading = false

    private var run: RunLogSummary?
    private weak var model: AppModel?
    private var requestToken = 0
    private var listNextCursor: String?
    private var listReachedEnd = false
    private var treeNextCursor: [String: String?] = [:]
    private var treeReachedEnd = Set<String>()
    private var treeLoadingParents = Set<String>()
    private var blockNextCursor: String?
    private var blocksReachedEnd = false
    private var blockRequestEpoch = SnapshotBlockRequestEpoch()
    private var activePresentation: SnapshotInspectionPresentation = .tree
    private var activeChangesOnly = true
    private var activeQuery = ""
    private var activeBlockChangesOnly = false
    private var storageNextCursor: String?
    private var storageReachedEnd = false
    private var storageBlockNextCursor: [String: String?] = [:]
    private var storageBlockReachedEnd = Set<String>()
    private var storageBlockLoading = Set<String>()
    private var storageRequestEpoch = SnapshotBlockRequestEpoch()
    private var activeStorageKind: String?
    private var activeStorageQuery = ""

    func start(run: RunLogSummary, model: AppModel) {
        self.run = run
        self.model = model
        requestToken += 1
        summary = nil
        issue = nil
        issueRetryable = false
        activeBlockChangesOnly = false
        activeStorageKind = nil
        activeStorageQuery = ""
        resetPagedContent()

        guard case .inspectable = SnapshotInspectionEligibility.forRun(run) else {
            issue = unavailableReason(for: run)
            return
        }
        if MainWindowUIDemo.enabled {
            installDemo(scene: MainWindowUIDemo.scene)
            return
        }
        loadSummary()
    }

    func retry() {
        guard let run, let model else { return }
        start(run: run, model: model)
    }

    func configureFiles(
        presentation: SnapshotInspectionPresentation,
        changesOnly: Bool,
        query: String
    ) {
        guard activePresentation != presentation || activeChangesOnly != changesOnly || activeQuery != query else {
            return
        }
        activePresentation = presentation
        activeChangesOnly = changesOnly
        activeQuery = query
        resetPagedContent()
        loadInitialFiles()
    }

    func loadInitialFiles() {
        guard summary != nil, issue == nil else { return }
        if activePresentation == .tree {
            loadTreeChildren(parent: "")
        } else {
            loadListPage()
        }
    }

    func loadMoreList() {
        guard activePresentation == .list, !listReachedEnd, !filesLoading else { return }
        loadListPage()
    }

    func loadTreeChildren(parent: String) {
        guard activePresentation == .tree,
              !treeReachedEnd.contains(parent),
              !treeLoadingParents.contains(parent),
              summary != nil,
              issue == nil
        else { return }
        treeLoadingParents.insert(parent)
        filesLoading = true
        let cursor = treeNextCursor[parent] ?? nil
        let token = requestToken
        performFilesRequest(parent: parent.isEmpty ? nil : parent, cursor: cursor) { [weak self] result in
            guard let self, self.requestToken == token else { return }
            self.treeLoadingParents.remove(parent)
            self.filesLoading = !self.treeLoadingParents.isEmpty
            switch result {
            case let .success(page):
                self.treeEntries[parent, default: []].append(contentsOf: page.entries)
                self.treeNextCursor[parent] = page.nextCursor
                if page.nextCursor == nil { self.treeReachedEnd.insert(parent) }
            case let .failure(failure):
                self.issue = failure.message
                self.issueRetryable = failure.retryable
            }
        }
    }

    func loadBlocksIfNeeded() {
        guard summary != nil, issue == nil, blocks.isEmpty, !blocksLoading else { return }
        loadBlockPage()
    }

    func configureBlocks(changesOnly: Bool) {
        guard activeBlockChangesOnly != changesOnly else { return }
        activeBlockChangesOnly = changesOnly
        resetBlockContent()
        loadBlocksIfNeeded()
    }

    func loadMoreBlocks() {
        guard !blocksReachedEnd, !blocksLoading else { return }
        loadBlockPage()
    }

    func loadStorageIfNeeded() {
        guard summary != nil, issue == nil, storageEntries.isEmpty, !storageLoading else { return }
        loadStoragePage()
    }

    func configureStorage(kind: String?, query: String) {
        guard activeStorageKind != kind || activeStorageQuery != query else { return }
        activeStorageKind = kind
        activeStorageQuery = query
        resetStorageContent()
        loadStorageIfNeeded()
    }

    func loadMoreStorage() {
        guard !storageReachedEnd, !storageLoading else { return }
        loadStoragePage()
    }

    func toggleStorage(_ entry: SnapshotStorageEntry) {
        if storageBlocks[entry.id] != nil {
            storageBlocks[entry.id] = nil
            storageBlockReachedEnd.remove(entry.id)
            storageBlockNextCursor[entry.id] = nil
            return
        }
        loadStorageBlocks(entry.id)
    }

    var loadingStorageObjectIDs: Set<String> { storageBlockLoading }

    var changesAvailable: Bool {
        summary?.availability.state != "baselineUnavailable"
    }

    var blockChangesAvailable: Bool {
        summary?.availability.state != "baselineUnavailable"
    }

    private func unavailableReason(for run: RunLogSummary) -> String {
        switch SnapshotInspectionEligibility.forRun(run) {
        case .inspectable:
            return ""
        case let .unavailable(reason):
            return reason
        }
    }

    private func loadSummary() {
        guard let snapshotId = run?.snapshotId, let model else { return }
        summaryLoading = true
        let token = requestToken
        performControlRequest(
            model: model,
            method: "snapshot.inspect.summary",
            params: ["snapshotId": snapshotId]
        ) {
            (result: Result<SnapshotInspectionSummary, ControlRequestFailure>) in
            guard self.requestToken == token else { return }
            self.summaryLoading = false
            switch result {
            case let .success(summary):
                self.summary = summary
                self.activeChangesOnly = summary.availability.state != "baselineUnavailable"
            case let .failure(failure):
                self.issue = failure.message
                self.issueRetryable = failure.retryable
            }
        }
    }

    private func loadListPage() {
        guard !filesLoading, !listReachedEnd else { return }
        filesLoading = true
        let cursor = listNextCursor
        let token = requestToken
        performFilesRequest(parent: nil, cursor: cursor) { [weak self] result in
            guard let self, self.requestToken == token else { return }
            self.filesLoading = false
            switch result {
            case let .success(page):
                self.listEntries.append(contentsOf: page.entries)
                self.listNextCursor = page.nextCursor
                self.listReachedEnd = page.nextCursor == nil
            case let .failure(failure):
                self.issue = failure.message
                self.issueRetryable = failure.retryable
            }
        }
    }

    private func performFilesRequest(
        parent: String?,
        cursor: String?,
        completion: @escaping (Result<SnapshotFilePage, ControlRequestFailure>) -> Void
    ) {
        guard let snapshotId = run?.snapshotId, let model else { return }
        var params: [String: Any] = [
            "snapshotId": snapshotId,
            "presentation": activePresentation.commandValue,
            "scope": activeChangesOnly ? "changes" : "all",
            "limit": 200,
        ]
        if let parent { params["parent"] = parent }
        if !activeQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            params["query"] = activeQuery
        }
        if let cursor { params["cursor"] = cursor }
        performControlRequest(
            model: model,
            method: "snapshot.inspect.files",
            params: params,
            completion: completion
        )
    }

    private func loadBlockPage() {
        guard !blocksLoading, !blocksReachedEnd, let snapshotId = run?.snapshotId, let model else { return }
        blocksLoading = true
        let token = blockRequestEpoch.issue()
        var params: [String: Any] = [
            "snapshotId": snapshotId,
            "changesOnly": activeBlockChangesOnly,
            "limit": 200,
        ]
        if let blockNextCursor { params["cursor"] = blockNextCursor }
        performControlRequest(
            model: model,
            method: "snapshot.inspect.blocks",
            params: params
        ) { (result: Result<SnapshotBlockPage, ControlRequestFailure>) in
            guard self.blockRequestEpoch.accepts(token) else { return }
            self.blocksLoading = false
            switch result {
            case let .success(page):
                self.blocks.append(contentsOf: page.entries)
                self.blockNextCursor = page.nextCursor
                self.blocksReachedEnd = page.nextCursor == nil
            case let .failure(failure):
                self.issue = failure.message
                self.issueRetryable = failure.retryable
            }
        }
    }

    private func loadStoragePage() {
        guard !storageLoading, !storageReachedEnd, let snapshotId = run?.snapshotId, let model else { return }
        storageLoading = true
        let token = storageRequestEpoch.issue()
        var params: [String: Any] = ["snapshotId": snapshotId, "limit": 200]
        if let activeStorageKind { params["kind"] = activeStorageKind }
        if !activeStorageQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { params["query"] = activeStorageQuery }
        if let storageNextCursor { params["cursor"] = storageNextCursor }
        performControlRequest(model: model, method: "snapshot.inspect.storage", params: params) { (result: Result<SnapshotStoragePage, ControlRequestFailure>) in
            guard self.storageRequestEpoch.accepts(token) else { return }
            self.storageLoading = false
            switch result {
            case let .success(page):
                self.storageEntries.append(contentsOf: page.entries)
                self.storageNextCursor = page.nextCursor
                self.storageReachedEnd = page.nextCursor == nil
            case let .failure(failure):
                self.issue = failure.message
                self.issueRetryable = failure.retryable
            }
        }
    }

    private func loadStorageBlocks(_ storageId: String) {
        guard !storageBlockLoading.contains(storageId), !storageBlockReachedEnd.contains(storageId), let snapshotId = run?.snapshotId, let model else { return }
        storageBlockLoading.insert(storageId)
        let token = storageRequestEpoch.issue()
        var params: [String: Any] = ["snapshotId": snapshotId, "storageId": storageId, "limit": 200]
        if let cursor = storageBlockNextCursor[storageId] ?? nil { params["cursor"] = cursor }
        performControlRequest(model: model, method: "snapshot.inspect.storage-blocks", params: params) { (result: Result<SnapshotStorageBlocksPage, ControlRequestFailure>) in
            guard self.storageRequestEpoch.accepts(token) else { return }
            self.storageBlockLoading.remove(storageId)
            switch result {
            case let .success(page):
                self.storageBlocks[storageId, default: []].append(contentsOf: page.entries)
                self.storageBlockNextCursor[storageId] = page.nextCursor
                if page.nextCursor == nil { self.storageBlockReachedEnd.insert(storageId) }
            case let .failure(failure):
                self.issue = failure.message
                self.issueRetryable = failure.retryable
            }
        }
    }

    private func resetPagedContent() {
        resetFilesContent()
        resetBlockContent()
        resetStorageContent()
    }

    private func resetFilesContent() {
        listEntries = []
        treeEntries = [:]
        listNextCursor = nil
        listReachedEnd = false
        treeNextCursor = [:]
        treeReachedEnd = []
        treeLoadingParents = []
        filesLoading = false
    }

    private func resetBlockContent() {
        _ = blockRequestEpoch.issue()
        blocks = []
        blockNextCursor = nil
        blocksReachedEnd = false
        blocksLoading = false
    }

    private func resetStorageContent() {
        _ = storageRequestEpoch.issue()
        storageEntries = []
        storageBlocks = [:]
        storageNextCursor = nil
        storageReachedEnd = false
        storageBlockNextCursor = [:]
        storageBlockReachedEnd = []
        storageBlockLoading = []
        storageLoading = false
    }

    private func performControlRequest<Response: Decodable>(
        model: AppModel,
        method: String,
        params: [String: Any],
        completion: @escaping (Result<Response, ControlRequestFailure>) -> Void
    ) {
        guard model.ensureDaemonRunning() else {
            completion(.failure(.init(code: "control.unavailable", message: "TelevyBackup service is unavailable.", retryable: true)))
            return
        }
        let socketPath = model.controlSocketPath()
        DispatchQueue.global(qos: .userInitiated).async {
            let decoded: Result<Response, ControlRequestFailure> = ControlIPCClient.request(
                socketPath: socketPath,
                method: method,
                params: params
            )
            DispatchQueue.main.async { completion(decoded) }
        }
    }

    private func installDemo(scene: String) {
        let unavailable = scene == "main-window-snapshot-baseline-unavailable"
        summary = SnapshotInspectionSummary(
            snapshot: .init(
                snapshotId: "s_demo_001",
                createdAt: "2026-08-27T00:00:00Z",
                sourcePath: "/Volumes/Demo/Photos",
                label: "Photos",
                baseSnapshotId: unavailable ? "s_demo_pruned" : "s_demo_000"
            ),
            availability: .init(
                state: unavailable ? "baselineUnavailable" : "available",
                reason: unavailable ? "The direct baseline is no longer retained." : nil
            ),
            files: .init(entries: 1_248, regularFiles: 1_120, directories: 119, symlinks: 9, bytes: 9_876_543_210),
            changes: .init(state: unavailable ? "baselineUnavailable" : "available", added: unavailable ? 0 : 14, deleted: unavailable ? 0 : 3, changed: unavailable ? 0 : 7),
            blocks: .init(distinct: 734, bytes: 9_876_543_210)
        )
        activeChangesOnly = !unavailable
        let tree: [SnapshotFileEntry]
        let albumChildren: [SnapshotFileEntry]
        if unavailable {
            tree = [
                SnapshotFileEntry(path: "Albums", name: "Albums", kind: "dir", change: "unchanged", isAncestorContext: false, size: 0, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: nil),
                SnapshotFileEntry(path: "Library.photoslibrary", name: "Library.photoslibrary", kind: "dir", change: "unchanged", isAncestorContext: false, size: 0, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: nil),
                SnapshotFileEntry(path: "new-import.jpg", name: "new-import.jpg", kind: "file", change: "unchanged", isAncestorContext: false, size: 4_120_332, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: nil),
                SnapshotFileEntry(path: "original-edit.jpg", name: "original-edit.jpg", kind: "file", change: "unchanged", isAncestorContext: false, size: 2_005_120, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: nil),
            ]
            albumChildren = [
                SnapshotFileEntry(path: "Albums/2026-08-27.jpg", name: "2026-08-27.jpg", kind: "file", change: "unchanged", isAncestorContext: false, size: 4_120_332, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: nil),
            ]
        } else {
            tree = [
                SnapshotFileEntry(path: "Albums", name: "Albums", kind: "dir", change: "unchanged", isAncestorContext: true, size: 0, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: .init(added: 8, deleted: 2, changed: 3)),
                SnapshotFileEntry(path: "Library.photoslibrary", name: "Library.photoslibrary", kind: "dir", change: "changed", isAncestorContext: false, size: 0, mtimeMs: 0, mode: 0, baseline: .init(kind: "dir", size: 0, mtimeMs: 0, mode: 0), descendantChanges: .init(added: 6, deleted: 1, changed: 4)),
                SnapshotFileEntry(path: "new-import.jpg", name: "new-import.jpg", kind: "file", change: "added", isAncestorContext: false, size: 4_120_332, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: nil),
                SnapshotFileEntry(path: "removed-edit.jpg", name: "removed-edit.jpg", kind: "file", change: "deleted", isAncestorContext: false, size: 2_005_120, mtimeMs: 0, mode: 0, baseline: .init(kind: "file", size: 2_005_120, mtimeMs: 0, mode: 0), descendantChanges: nil),
            ]
            albumChildren = [
                SnapshotFileEntry(path: "Albums/2026-08-27.jpg", name: "2026-08-27.jpg", kind: "file", change: "added", isAncestorContext: false, size: 4_120_332, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: nil),
                SnapshotFileEntry(path: "Albums/old-edit.jpg", name: "old-edit.jpg", kind: "file", change: "deleted", isAncestorContext: false, size: 2_005_120, mtimeMs: 0, mode: 0, baseline: .init(kind: "file", size: 2_005_120, mtimeMs: 0, mode: 0), descendantChanges: nil),
            ]
        }
        treeEntries = ["": tree, "Albums": albumChildren]
        listEntries = tree + albumChildren
        blocks = [
            SnapshotBlockEntry(hash: "9c47a0f53d1a6cb9", size: 1_048_576, changedFiles: 3, referencingFiles: 4),
            SnapshotBlockEntry(hash: "b9d202d17d25e8f1", size: 786_432, changedFiles: 0, referencingFiles: 2),
        ]
        storageEntries = [
            SnapshotStorageEntry(storageId: "sto_3e7a8f1c2d", kind: "pack", documentBytes: 64 * 1_024 * 1_024, recordedAt: "2026-08-27T00:00:01Z", referencedBlocks: 42, logicalBytes: 40 * 1_024 * 1_024),
            SnapshotStorageEntry(storageId: "sto_legacy_91b4", kind: "direct", documentBytes: nil, recordedAt: nil, referencedBlocks: 1, logicalBytes: 786_432),
        ]
        storageBlocks = [
            "sto_3e7a8f1c2d": [
                SnapshotStorageBlockEntry(hash: "9c47a0f53d1a6cb9", size: 1_048_576, offset: 0, length: 1_048_617),
                SnapshotStorageBlockEntry(hash: "a102b0c4d5e6f789", size: 786_432, offset: 1_048_617, length: 786_473),
            ],
        ]
        listReachedEnd = true
        treeReachedEnd = ["", "Albums"]
        blocksReachedEnd = true
        storageReachedEnd = true
    }
}

struct SnapshotRunDetailView: View {
    @Environment(\.appRuntime) private var model
    let run: RunLogSummary
    let onBack: () -> Void

    private enum Tab: String, CaseIterable, Identifiable {
        case summary = "Summary"
        case files = "Files"
        case blocks = "Blocks"
        case storage = "Storage"

        var id: String { rawValue }
    }

    @StateObject private var store = SnapshotInspectionStore()
    @State private var tab: Tab = {
        let scene = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_SCENE"] ?? ""
        return scene == "main-window-snapshot-changes" || scene == "main-window-snapshot-baseline-unavailable"
            ? .files
            : scene == "main-window-snapshot-storage" ? .storage
            : .summary
    }()
    @State private var presentation: SnapshotInspectionPresentation = .tree
    @State private var changesOnly = true
    @State private var blockChangesOnly = false
    @State private var query = ""
    @State private var storageKind: String?
    @State private var storageQuery = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            detailToolbar
            Divider()
            content
        }
        .padding(12)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .onAppear {
            store.start(run: run, model: model)
        }
        .onChange(of: tab) { _, newTab in
            if newTab == .blocks { store.loadBlocksIfNeeded() }
            if newTab == .files { store.loadInitialFiles() }
            if newTab == .storage { store.loadStorageIfNeeded() }
        }
        .onChange(of: presentation) { _, _ in reloadFiles() }
        .onChange(of: changesOnly) { _, _ in reloadFiles() }
        .onChange(of: blockChangesOnly) { _, _ in reloadBlocks() }
        .onChange(of: query) { _, _ in reloadFiles() }
        .onChange(of: storageKind) { _, _ in reloadStorage() }
        .onChange(of: storageQuery) { _, _ in reloadStorage() }
    }

    @ViewBuilder
    private var detailToolbar: some View {
        if tab == .files, let summary = store.summary, store.issue == nil {
            ViewThatFits(in: .horizontal) {
                wideFileToolbar(summary: summary)
                stackedFileToolbar(summary: summary)
                compactFileToolbar(summary: summary)
            }
        } else if tab == .blocks, let summary = store.summary, store.issue == nil {
            ViewThatFits(in: .horizontal) {
                wideBlockToolbar(summary: summary)
                stackedBlockToolbar(summary: summary)
            }
        } else if tab == .storage, store.issue == nil {
            ViewThatFits(in: .horizontal) {
                wideStorageToolbar
                stackedStorageToolbar
            }
        } else {
            tabPicker
        }
    }

    private func wideFileToolbar(summary: SnapshotInspectionSummary) -> some View {
        HStack(spacing: 16) {
            tabPicker
            Spacer(minLength: 24)
            fileControls(summary: summary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func stackedFileToolbar(summary: SnapshotInspectionSummary) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            tabPicker
            HStack(spacing: 8) {
                fileControls(summary: summary)
                Spacer(minLength: 0)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func compactFileToolbar(summary: SnapshotInspectionSummary) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            tabPicker
            HStack(spacing: 8) {
                filePresentationPicker
                changesOnlyToggle
                Spacer(minLength: 0)
            }
            fileSearchField.frame(maxWidth: .infinity)
            availabilityNotice(summary: summary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func wideBlockToolbar(summary: SnapshotInspectionSummary) -> some View {
        HStack(spacing: 16) {
            tabPicker
            Spacer(minLength: 24)
            blockControls(summary: summary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func stackedBlockToolbar(summary: SnapshotInspectionSummary) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            tabPicker
            blockControls(summary: summary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func blockControls(summary: SnapshotInspectionSummary) -> some View {
        HStack(spacing: 8) {
            blockChangesOnlyToggle
            blockAvailabilityNotice(summary: summary)
        }
    }

    private var wideStorageToolbar: some View {
        HStack(spacing: 16) {
            tabPicker
            Spacer(minLength: 24)
            storageControls
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var stackedStorageToolbar: some View {
        VStack(alignment: .leading, spacing: 8) {
            tabPicker
            storageControls
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var storageControls: some View {
        HStack(spacing: 8) {
            Picker("Storage kind", selection: $storageKind) {
                Text("All").tag(String?.none)
                Text("Pack").tag(String?.some("pack"))
                Text("Direct").tag(String?.some("direct"))
            }
            .labelsHidden()
            .pickerStyle(.segmented)
            .controlSize(.small)
            .frame(width: 158)
            TextField("Search object ID", text: $storageQuery)
                .textFieldStyle(.roundedBorder)
                .frame(minWidth: 150, idealWidth: 210, maxWidth: 260)
        }
    }

    private var blockChangesOnlyToggle: some View {
        Toggle("Changes only", isOn: $blockChangesOnly)
            .toggleStyle(.checkbox)
            .controlSize(.small)
            .font(.system(size: 11, weight: .medium))
            .disabled(!store.blockChangesAvailable)
            .help("Show blocks referenced by added or changed files.")
    }

    @ViewBuilder
    private func blockAvailabilityNotice(summary: SnapshotInspectionSummary) -> some View {
        if summary.availability.state == "baselineUnavailable" {
            Label("Baseline unavailable", systemImage: "exclamationmark.triangle.fill")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .help("Changed block counts require the direct base snapshot.")
                .accessibilityLabel("Direct baseline unavailable for changed blocks")
        }
    }

    private var tabPicker: some View {
        Picker("", selection: $tab) {
            ForEach(Tab.allCases) { tab in Text(tab.rawValue).tag(tab) }
        }
        .pickerStyle(.segmented)
        .controlSize(.small)
        .frame(width: 280)
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            Button(action: onBack) {
                Image(systemName: "chevron.backward")
            }
            .buttonStyle(.borderless)
            .help("Back to history")
            Text("\(run.kind.capitalized) details")
                .font(.system(size: 18, weight: .bold))
            if let status = run.status {
                Text(status.capitalized)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(statusColor)
            }
            Spacer(minLength: 0)
            if let snapshotId = run.snapshotId {
                Text(snapshotId)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                Button {
                    NSPasteboard.general.clearContents()
                    NSPasteboard.general.setString(snapshotId, forType: .string)
                } label: {
                    Image(systemName: "doc.on.doc")
                }
                .buttonStyle(.borderless)
                .help("Copy snapshot ID")
                .accessibilityLabel("Copy snapshot ID")
            }
        }
    }

    private var statusColor: Color {
        switch run.status {
        case "succeeded": .green
        case "failed": .red
        case "running": .blue
        default: .secondary
        }
    }

    @ViewBuilder
    private var content: some View {
        if store.summaryLoading {
            SnapshotInspectionStateView(icon: "arrow.triangle.2.circlepath", title: "Loading snapshot", detail: "Reading the retained file map.", showsProgress: true)
        } else if let issue = store.issue {
            VStack(alignment: .leading, spacing: 12) {
                SnapshotExecutionSummaryView(run: run)
                Divider()
                SnapshotInspectionStateView(icon: "exclamationmark.triangle", title: "Snapshot unavailable", detail: issue, showsProgress: false) {
                    if store.issueRetryable {
                        Button("Retry") { store.retry() }
                            .controlSize(.small)
                    }
                }
            }
        } else if let summary = store.summary {
            switch tab {
            case .summary:
                SnapshotSummaryView(run: run, summary: summary)
            case .files:
                files(summary: summary)
            case .blocks:
                blocks(summary: summary)
            case .storage:
                storage()
            }
        } else {
            SnapshotInspectionStateView(icon: "doc.text", title: "No snapshot data", detail: "This run has no inspectable retained snapshot.", showsProgress: false)
        }
    }

    private func files(summary: SnapshotInspectionSummary) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if presentation == .tree, store.treeEntries["", default: []].isEmpty, !store.filesLoading {
                SnapshotInspectionStateView(icon: "folder", title: "No files", detail: "No retained paths match this view.", showsProgress: false)
            } else if presentation == .tree {
                SnapshotOutlineTable(entriesByParent: store.treeEntries, onExpand: { store.loadTreeChildren(parent: $0) })
            } else if store.listEntries.isEmpty, !store.filesLoading {
                SnapshotInspectionStateView(icon: "doc", title: "No files", detail: "No retained paths match this view.", showsProgress: false)
            } else {
                SnapshotFileTable(entries: store.listEntries, onReachedBottom: { store.loadMoreList() })
            }
            if store.filesLoading {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("Loading files…").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .onAppear {
            if summary.availability.state == "baselineUnavailable" { changesOnly = false }
            reloadFiles()
        }
    }

    private func fileControls(summary: SnapshotInspectionSummary) -> some View {
        HStack(spacing: 8) {
            filePresentationPicker
            changesOnlyToggle
            fileSearchField
            availabilityNotice(summary: summary)
        }
    }

    private var filePresentationPicker: some View {
        Picker("File presentation", selection: $presentation) {
            ForEach(SnapshotInspectionPresentation.allCases) { item in Text(item.rawValue).tag(item) }
        }
        .labelsHidden()
        .pickerStyle(.segmented)
        .controlSize(.small)
        .frame(width: 98)
    }

    private var changesOnlyToggle: some View {
        Toggle("Changes only", isOn: $changesOnly)
            .toggleStyle(.checkbox)
            .controlSize(.small)
            .font(.system(size: 11, weight: .medium))
            .disabled(!store.changesAvailable)
    }

    private var fileSearchField: some View {
        TextField("Search paths", text: $query)
            .textFieldStyle(.roundedBorder)
            .frame(minWidth: 125, idealWidth: 180, maxWidth: 250)
    }

    @ViewBuilder
    private func availabilityNotice(summary: SnapshotInspectionSummary) -> some View {
        if summary.availability.state == "baselineUnavailable" {
            Label("Baseline unavailable", systemImage: "exclamationmark.triangle.fill")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)
                .lineLimit(1)
                .help("The direct base snapshot is no longer retained.")
                .accessibilityLabel("Direct baseline unavailable")
        }
    }

    private func blocks(summary: SnapshotInspectionSummary) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if store.blocks.isEmpty, !store.blocksLoading {
                SnapshotInspectionStateView(icon: "square.stack.3d.up", title: "No blocks", detail: "This snapshot has no regular-file blocks.", showsProgress: false)
            } else {
                SnapshotBlockTable(entries: store.blocks, onReachedBottom: { store.loadMoreBlocks() })
            }
            if store.blocksLoading {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("Loading blocks…").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .onAppear {
            if summary.availability.state == "baselineUnavailable" { blockChangesOnly = false }
            reloadBlocks()
        }
    }

    private func storage() -> some View {
        VStack(alignment: .leading, spacing: 10) {
            if store.storageEntries.isEmpty, !store.storageLoading {
                SnapshotInspectionStateView(icon: "externaldrive", title: "No storage objects", detail: "No Pack or Direct documents are referenced by this snapshot.", showsProgress: false)
            } else {
                SnapshotStorageTable(
                    entries: store.storageEntries,
                    expandedBlocks: store.storageBlocks,
                    loadingObjectIDs: store.loadingStorageObjectIDs,
                    onReachedBottom: { store.loadMoreStorage() },
                    onToggle: { store.toggleStorage($0) }
                )
            }
            if store.storageLoading {
                HStack(spacing: 6) {
                    ProgressView().controlSize(.small)
                    Text("Loading storage objects...").font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
                }
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .onAppear { reloadStorage() }
    }

    private func reloadFiles() {
        if store.summary != nil {
            store.configureFiles(presentation: presentation, changesOnly: changesOnly, query: query)
        }
    }

    private func reloadBlocks() {
        guard store.summary != nil else { return }
        store.configureBlocks(changesOnly: blockChangesOnly)
        store.loadBlocksIfNeeded()
    }

    private func reloadStorage() {
        guard store.summary != nil else { return }
        store.configureStorage(kind: storageKind, query: storageQuery)
        store.loadStorageIfNeeded()
    }
}

private struct SnapshotSummaryView: View {
    let run: RunLogSummary
    let summary: SnapshotInspectionSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Grid(alignment: .leading, horizontalSpacing: 28, verticalSpacing: 8) {
                GridRow { label("Outcome"); Text((run.status ?? "unknown").capitalized) }
                GridRow { label("Started"); Text(run.startedAt.map(timestamp) ?? "Unavailable") }
                GridRow { label("Finished"); Text(run.finishedAt.map(timestamp) ?? "Unavailable") }
                GridRow { label("Duration"); Text(run.durationSeconds.map(formatDuration) ?? "Unavailable") }
                GridRow { label("Uploaded"); Text(run.bytesUploaded.map(formatBytes) ?? "Unavailable") }
                GridRow { label("Deduped"); Text(run.bytesDeduped.map(formatBytes) ?? "Unavailable") }
                GridRow { label("Error"); Text(errorText) }
                GridRow { label("Source"); Text(summary.snapshot.sourcePath).lineLimit(1).truncationMode(.middle) }
                GridRow { label("Snapshot"); Text(summary.snapshot.snapshotId).font(.system(size: 11, design: .monospaced)) }
                GridRow { label("Baseline"); Text(summary.availability.state == "baselineUnavailable" ? "Unavailable" : (summary.snapshot.baseSnapshotId ?? "First snapshot")) }
            }
            Divider()
            HStack(alignment: .top, spacing: 28) {
                summaryMetric("Files", "\(summary.files.entries)", "doc.on.doc")
                summaryMetric("Changed", "\(summary.changes.changed)", "pencil.circle.fill")
                summaryMetric("Added", "\(summary.changes.added)", "plus.circle.fill")
                summaryMetric("Deleted", "\(summary.changes.deleted)", "minus.circle.fill")
                summaryMetric("Blocks", "\(summary.blocks.distinct)", "square.stack.3d.up")
            }
            if let reason = summary.availability.reason {
                Text(reason)
                    .font(.system(size: 12, weight: .medium))
                    .foregroundStyle(.secondary)
            }
            Spacer(minLength: 0)
        }
    }

    private func label(_ text: String) -> some View {
        Text(text).font(.system(size: 12, weight: .semibold)).foregroundStyle(.secondary)
    }

    private func timestamp(_ date: Date) -> String {
        date.formatted(date: .abbreviated, time: .standard)
    }

    private var errorText: String {
        run.errorCode.flatMap { $0.isEmpty ? nil : $0 } ?? "None"
    }

    private func summaryMetric(_ title: String, _ value: String, _ icon: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Image(systemName: icon).foregroundStyle(.secondary)
            Text(value).font(.system(size: 16, weight: .bold))
            Text(title).font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
        }
        .frame(minWidth: 72, alignment: .leading)
    }
}

private struct SnapshotExecutionSummaryView: View {
    let run: RunLogSummary

    var body: some View {
        HStack(alignment: .top, spacing: 16) {
            Grid(alignment: .leading, horizontalSpacing: 24, verticalSpacing: 7) {
                GridRow { label("Run"); Text(run.kind.capitalized) }
                GridRow { label("Outcome"); Text((run.status ?? "unknown").capitalized) }
                GridRow { label("Started"); Text(run.startedAt.map(timestamp) ?? "Unavailable") }
                GridRow { label("Finished"); Text(run.finishedAt.map(timestamp) ?? "Unavailable") }
                GridRow { label("Duration"); Text(run.durationSeconds.map(formatDuration) ?? "Unavailable") }
                GridRow { label("Error"); Text(errorText) }
                GridRow { label("Source"); Text(run.sourcePath ?? "Unavailable").lineLimit(1).truncationMode(.middle) }
            }
            Spacer(minLength: 0)
            Button {
                NSWorkspace.shared.activateFileViewerSelecting([run.logURL])
            } label: {
                Image(systemName: "doc.text")
            }
            .buttonStyle(.borderless)
            .help("Reveal log file in Finder")
            .accessibilityLabel("Reveal log file in Finder")
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Run execution summary")
    }

    private func label(_ text: String) -> some View {
        Text(text).font(.system(size: 12, weight: .semibold)).foregroundStyle(.secondary)
    }

    private func timestamp(_ date: Date) -> String {
        date.formatted(date: .abbreviated, time: .standard)
    }

    private var errorText: String {
        run.errorCode.flatMap { $0.isEmpty ? nil : $0 } ?? "None"
    }
}

private struct SnapshotInspectionStateView<Actions: View>: View {
    let icon: String
    let title: String
    let detail: String
    let showsProgress: Bool
    @ViewBuilder let actions: () -> Actions

    init(icon: String, title: String, detail: String, showsProgress: Bool, @ViewBuilder actions: @escaping () -> Actions = { EmptyView() }) {
        self.icon = icon
        self.title = title
        self.detail = detail
        self.showsProgress = showsProgress
        self.actions = actions
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 9) {
                Image(systemName: icon).foregroundStyle(.secondary)
                Text(title).font(.system(size: 14, weight: .bold))
                if showsProgress { ProgressView().controlSize(.small) }
            }
            Text(detail).font(.system(size: 12, weight: .medium)).foregroundStyle(.secondary)
            actions()
            Spacer(minLength: 0)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
    }
}

enum SnapshotOutlineExpansion {
    static func pathsToRestore(
        previouslyExpanded: Set<String>,
        availablePaths: Set<String>
    ) -> Set<String> {
        previouslyExpanded.intersection(availablePaths)
    }
}

private final class SnapshotOutlineNode: NSObject {
    let entry: SnapshotFileEntry
    var children: [SnapshotOutlineNode]

    init(entry: SnapshotFileEntry, children: [SnapshotOutlineNode]) {
        self.entry = entry
        self.children = children
    }
}

private enum SnapshotNativeColumns {
    enum File {
        static let name = NSUserInterfaceItemIdentifier("snapshot-file-name")
        static let change = NSUserInterfaceItemIdentifier("snapshot-file-change")
        static let size = NSUserInterfaceItemIdentifier("snapshot-file-size")

        static func install(on table: NSTableView) {
            table.headerView = NSTableHeaderView()
            table.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
            table.addTableColumn(column(title: "Name", identifier: name, width: 420, minWidth: 180, expands: true, alignment: .left))
            table.addTableColumn(column(title: "Change", identifier: change, width: 132, minWidth: 112, expands: false, alignment: .left))
            table.addTableColumn(column(title: "Size", identifier: size, width: 92, minWidth: 76, expands: false, alignment: .right))
        }
    }

    enum Block {
        static let hash = NSUserInterfaceItemIdentifier("snapshot-block-hash")
        static let size = NSUserInterfaceItemIdentifier("snapshot-block-size")
        static let changedFiles = NSUserInterfaceItemIdentifier("snapshot-block-changed-files")
        static let referencedFiles = NSUserInterfaceItemIdentifier("snapshot-block-referenced-files")

        static func install(on table: NSTableView) {
            table.headerView = NSTableHeaderView()
            table.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
            table.addTableColumn(column(title: "Hash", identifier: hash, width: 420, minWidth: 180, expands: true, alignment: .left))
            table.addTableColumn(column(title: "Size", identifier: size, width: 108, minWidth: 84, expands: false, alignment: .right))
            table.addTableColumn(column(title: "Changed files", identifier: changedFiles, width: 120, minWidth: 112, expands: false, alignment: .right))
            table.addTableColumn(column(title: "Referenced files", identifier: referencedFiles, width: 136, minWidth: 124, expands: false, alignment: .right))
        }
    }

    enum Storage {
        static let object = NSUserInterfaceItemIdentifier("snapshot-storage-object")
        static let kind = NSUserInterfaceItemIdentifier("snapshot-storage-kind")
        static let document = NSUserInterfaceItemIdentifier("snapshot-storage-document")
        static let logical = NSUserInterfaceItemIdentifier("snapshot-storage-logical")
        static let references = NSUserInterfaceItemIdentifier("snapshot-storage-references")
        static let recorded = NSUserInterfaceItemIdentifier("snapshot-storage-recorded")

        static func install(on table: NSTableView) {
            table.headerView = NSTableHeaderView()
            table.columnAutoresizingStyle = .firstColumnOnlyAutoresizingStyle
            table.addTableColumn(column(title: "Object", identifier: object, width: 210, minWidth: 180, expands: true, alignment: .left))
            table.addTableColumn(column(title: "Type", identifier: kind, width: 58, minWidth: 52, expands: false, alignment: .left))
            table.addTableColumn(column(title: "Document", identifier: document, width: 90, minWidth: 84, expands: false, alignment: .right))
            table.addTableColumn(column(title: "Logical", identifier: logical, width: 88, minWidth: 80, expands: false, alignment: .right))
            table.addTableColumn(column(title: "Blocks", identifier: references, width: 52, minWidth: 48, expands: false, alignment: .right))
            table.addTableColumn(column(title: "Recorded", identifier: recorded, width: 110, minWidth: 96, expands: false, alignment: .right))
        }
    }

    private static func column(
        title: String,
        identifier: NSUserInterfaceItemIdentifier,
        width: CGFloat,
        minWidth: CGFloat,
        expands: Bool,
        alignment: NSTextAlignment
    ) -> NSTableColumn {
        let column = NSTableColumn(identifier: identifier)
        column.headerCell.stringValue = title
        column.headerCell.font = .systemFont(ofSize: 11, weight: .semibold)
        column.headerCell.alignment = alignment
        column.width = width
        column.minWidth = minWidth
        column.resizingMask = expands ? [.autoresizingMask, .userResizingMask] : .userResizingMask
        return column
    }
}

private struct SnapshotOutlineTable: NSViewRepresentable {
    let entriesByParent: [String: [SnapshotFileEntry]]
    let onExpand: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let outline = NSOutlineView()
        outline.rowSizeStyle = .small
        outline.delegate = context.coordinator
        outline.dataSource = context.coordinator
        SnapshotNativeColumns.File.install(on: outline)
        outline.outlineTableColumn = outline.tableColumn(withIdentifier: SnapshotNativeColumns.File.name)
        outline.setAccessibilityLabel("Snapshot file tree")
        let scroll = SnapshotNativeTable.scrollView(table: outline)
        context.coordinator.outline = outline
        context.coordinator.onExpand = onExpand
        context.coordinator.entriesByParent = entriesByParent
        context.coordinator.reload()
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.onExpand = onExpand
        context.coordinator.entriesByParent = entriesByParent
        context.coordinator.reload()
    }

    final class Coordinator: NSObject, NSOutlineViewDataSource, NSOutlineViewDelegate {
        weak var outline: NSOutlineView?
        var entriesByParent: [String: [SnapshotFileEntry]] = [:]
        var rootNodes: [SnapshotOutlineNode] = []
        var onExpand: ((String) -> Void)?
        private var isRestoringExpansion = false

        func reload() {
            let previouslyExpanded = expandedPaths()
            rootNodes = makeNodes(entriesByParent[""] ?? [])
            guard let outline else { return }
            let pathsToRestore = SnapshotOutlineExpansion.pathsToRestore(
                previouslyExpanded: previouslyExpanded,
                availablePaths: allPaths(in: rootNodes)
            )
            outline.reloadData()
            restoreExpansion(paths: pathsToRestore, in: outline)
        }

        private func makeNodes(_ entries: [SnapshotFileEntry]) -> [SnapshotOutlineNode] {
            entries.map { entry in
                SnapshotOutlineNode(entry: entry, children: makeNodes(entriesByParent[entry.path] ?? []))
            }
        }

        private func expandedPaths() -> Set<String> {
            guard let outline else { return [] }
            return expandedPaths(in: rootNodes, outline: outline)
        }

        private func expandedPaths(
            in nodes: [SnapshotOutlineNode],
            outline: NSOutlineView
        ) -> Set<String> {
            nodes.reduce(into: Set<String>()) { paths, node in
                guard outline.isItemExpanded(node) else { return }
                paths.insert(node.entry.path)
                paths.formUnion(expandedPaths(in: node.children, outline: outline))
            }
        }

        private func allPaths(in nodes: [SnapshotOutlineNode]) -> Set<String> {
            nodes.reduce(into: Set<String>()) { paths, node in
                paths.insert(node.entry.path)
                paths.formUnion(allPaths(in: node.children))
            }
        }

        private func restoreExpansion(paths: Set<String>, in outline: NSOutlineView) {
            guard !paths.isEmpty else { return }
            isRestoringExpansion = true
            defer { isRestoringExpansion = false }
            restoreExpansion(paths: paths, in: rootNodes, outline: outline)
        }

        private func restoreExpansion(
            paths: Set<String>,
            in nodes: [SnapshotOutlineNode],
            outline: NSOutlineView
        ) {
            for node in nodes where paths.contains(node.entry.path) {
                outline.expandItem(node)
                restoreExpansion(paths: paths, in: node.children, outline: outline)
            }
        }

        func outlineView(_: NSOutlineView, numberOfChildrenOfItem item: Any?) -> Int {
            (item as? SnapshotOutlineNode)?.children.count ?? rootNodes.count
        }

        func outlineView(_: NSOutlineView, child index: Int, ofItem item: Any?) -> Any {
            if let item = item as? SnapshotOutlineNode { return item.children[index] }
            return rootNodes[index]
        }

        func outlineView(_: NSOutlineView, isItemExpandable item: Any) -> Bool {
            (item as? SnapshotOutlineNode)?.entry.kind == "dir"
        }

        func outlineViewItemDidExpand(_ notification: Notification) {
            guard !isRestoringExpansion else { return }
            if let node = notification.userInfo?["NSObject"] as? SnapshotOutlineNode {
                onExpand?(node.entry.path)
            }
        }

        func outlineView(_ outlineView: NSOutlineView, viewFor tableColumn: NSTableColumn?, item: Any) -> NSView? {
            guard let node = item as? SnapshotOutlineNode else { return nil }
            guard let tableColumn else { return nil }
            switch tableColumn.identifier {
            case SnapshotNativeColumns.File.name:
                let inset = CGFloat(outlineView.level(forItem: node)) * outlineView.indentationPerLevel
                return SnapshotNativeRowView.fileName(entry: node.entry, leadingInset: inset, usesOutlineLayout: true)
            case SnapshotNativeColumns.File.change:
                return SnapshotNativeRowView.fileChange(entry: node.entry)
            case SnapshotNativeColumns.File.size:
                return SnapshotNativeRowView.fileSize(entry: node.entry)
            default:
                return nil
            }
        }
    }
}

private struct SnapshotFileTable: NSViewRepresentable {
    let entries: [SnapshotFileEntry]
    let onReachedBottom: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let table = NSTableView()
        table.rowSizeStyle = .small
        table.delegate = context.coordinator
        table.dataSource = context.coordinator
        SnapshotNativeColumns.File.install(on: table)
        table.setAccessibilityLabel("Snapshot file list")
        let scroll = SnapshotNativeTable.scrollView(table: table, coordinator: context.coordinator)
        context.coordinator.table = table
        context.coordinator.onReachedBottom = onReachedBottom
        context.coordinator.entries = entries
        return scroll
    }

    func updateNSView(_: NSScrollView, context: Context) {
        context.coordinator.entries = entries
        context.coordinator.onReachedBottom = onReachedBottom
        context.coordinator.table?.reloadData()
    }

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate, SnapshotNativeTableObserver {
        weak var table: NSTableView?
        var entries: [SnapshotFileEntry] = []
        var onReachedBottom: (() -> Void)?

        func numberOfRows(in _: NSTableView) -> Int { entries.count }

        func tableView(_: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard let tableColumn else { return nil }
            let entry = entries[row]
            switch tableColumn.identifier {
            case SnapshotNativeColumns.File.name:
                return SnapshotNativeRowView.fileName(entry: entry, leadingInset: 0, usesOutlineLayout: false)
            case SnapshotNativeColumns.File.change:
                return SnapshotNativeRowView.fileChange(entry: entry)
            case SnapshotNativeColumns.File.size:
                return SnapshotNativeRowView.fileSize(entry: entry)
            default:
                return nil
            }
        }

        func visibleRowsApproachEnd() { onReachedBottom?() }
    }
}

private struct SnapshotBlockTable: NSViewRepresentable {
    let entries: [SnapshotBlockEntry]
    let onReachedBottom: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let table = NSTableView()
        table.rowSizeStyle = .small
        table.delegate = context.coordinator
        table.dataSource = context.coordinator
        SnapshotNativeColumns.Block.install(on: table)
        table.setAccessibilityLabel("Snapshot blocks")
        let scroll = SnapshotNativeTable.scrollView(table: table, coordinator: context.coordinator)
        context.coordinator.table = table
        context.coordinator.onReachedBottom = onReachedBottom
        context.coordinator.entries = entries
        return scroll
    }

    func updateNSView(_: NSScrollView, context: Context) {
        context.coordinator.entries = entries
        context.coordinator.onReachedBottom = onReachedBottom
        context.coordinator.table?.reloadData()
    }

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate, SnapshotNativeTableObserver {
        weak var table: NSTableView?
        var entries: [SnapshotBlockEntry] = []
        var onReachedBottom: (() -> Void)?

        func numberOfRows(in _: NSTableView) -> Int { entries.count }

        func tableView(_: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard let tableColumn else { return nil }
            let entry = entries[row]
            switch tableColumn.identifier {
            case SnapshotNativeColumns.Block.hash:
                return SnapshotNativeRowView.blockHash(entry: entry)
            case SnapshotNativeColumns.Block.size:
                return SnapshotNativeRowView.blockSize(entry: entry)
            case SnapshotNativeColumns.Block.changedFiles:
                return SnapshotNativeRowView.blockChangedFiles(entry: entry)
            case SnapshotNativeColumns.Block.referencedFiles:
                return SnapshotNativeRowView.blockReferencedFiles(entry: entry)
            default:
                return nil
            }
        }

        func visibleRowsApproachEnd() { onReachedBottom?() }
    }
}

private struct SnapshotStorageTable: NSViewRepresentable {
    let entries: [SnapshotStorageEntry]
    let expandedBlocks: [String: [SnapshotStorageBlockEntry]]
    let loadingObjectIDs: Set<String>
    let onReachedBottom: () -> Void
    let onToggle: (SnapshotStorageEntry) -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let table = NSTableView()
        table.rowSizeStyle = .small
        table.delegate = context.coordinator
        table.dataSource = context.coordinator
        SnapshotNativeColumns.Storage.install(on: table)
        table.setAccessibilityLabel("Snapshot storage objects")
        let scroll = SnapshotNativeTable.scrollView(table: table, coordinator: context.coordinator)
        context.coordinator.table = table
        context.coordinator.onReachedBottom = onReachedBottom
        context.coordinator.onToggle = onToggle
        context.coordinator.update(entries: entries, expandedBlocks: expandedBlocks, loadingObjectIDs: loadingObjectIDs)
        return scroll
    }

    func updateNSView(_: NSScrollView, context: Context) {
        context.coordinator.onReachedBottom = onReachedBottom
        context.coordinator.onToggle = onToggle
        context.coordinator.update(entries: entries, expandedBlocks: expandedBlocks, loadingObjectIDs: loadingObjectIDs)
    }

    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate, SnapshotNativeTableObserver {
        enum Row {
            case object(SnapshotStorageEntry)
            case block(SnapshotStorageBlockEntry)
        }

        weak var table: NSTableView?
        var rows: [Row] = []
        var storageEntriesByID: [String: SnapshotStorageEntry] = [:]
        var expandedStorageIDs: Set<String> = []
        var loadingStorageIDs: Set<String> = []
        var onReachedBottom: (() -> Void)?
        var onToggle: ((SnapshotStorageEntry) -> Void)?

        func update(entries: [SnapshotStorageEntry], expandedBlocks: [String: [SnapshotStorageBlockEntry]], loadingObjectIDs: Set<String>) {
            storageEntriesByID = Dictionary(uniqueKeysWithValues: entries.map { ($0.id, $0) })
            expandedStorageIDs = Set(expandedBlocks.keys)
            loadingStorageIDs = loadingObjectIDs
            rows = entries.flatMap { entry -> [Row] in
                var result: [Row] = [.object(entry)]
                if let blocks = expandedBlocks[entry.id] {
                    result.append(contentsOf: blocks.map(Row.block))
                } else if loadingObjectIDs.contains(entry.id) {
                    result.append(.block(SnapshotStorageBlockEntry(hash: "Loading slices...", size: 0, offset: 0, length: 0)))
                }
                return result
            }
            table?.reloadData()
        }

        func numberOfRows(in _: NSTableView) -> Int { rows.count }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            guard let tableColumn else { return nil }
            switch rows[row] {
            case let .object(entry):
                switch tableColumn.identifier {
                case SnapshotNativeColumns.Storage.object:
                    let isExpanded = expandedStorageIDs.contains(entry.id) || loadingStorageIDs.contains(entry.id)
                    return SnapshotNativeRowView.storageObject(
                        entry: entry,
                        isExpanded: isExpanded,
                        target: self,
                        action: #selector(toggleStorageObject(_:))
                    )
                case SnapshotNativeColumns.Storage.kind: return SnapshotNativeRowView.storageKind(entry: entry)
                case SnapshotNativeColumns.Storage.document: return SnapshotNativeRowView.storageDocument(entry: entry)
                case SnapshotNativeColumns.Storage.logical: return SnapshotNativeRowView.storageLogical(entry: entry)
                case SnapshotNativeColumns.Storage.references: return SnapshotNativeRowView.storageReferences(entry: entry)
                case SnapshotNativeColumns.Storage.recorded: return SnapshotNativeRowView.storageRecorded(entry: entry)
                default: return nil
                }
            case let .block(entry):
                switch tableColumn.identifier {
                case SnapshotNativeColumns.Storage.object: return SnapshotNativeRowView.storageBlock(entry: entry)
                case SnapshotNativeColumns.Storage.kind: return SnapshotNativeRowView.storageSliceType(entry: entry)
                case SnapshotNativeColumns.Storage.document: return SnapshotNativeRowView.storageSliceLength(entry: entry)
                case SnapshotNativeColumns.Storage.logical: return SnapshotNativeRowView.storageBlockSize(entry: entry)
                case SnapshotNativeColumns.Storage.references: return SnapshotNativeRowView.storageSliceOffset(entry: entry)
                case SnapshotNativeColumns.Storage.recorded: return SnapshotNativeRowView.empty()
                default: return nil
                }
            }
        }

        func tableViewSelectionDidChange(_ notification: Notification) {
            guard let table, table.selectedRow >= 0, table.selectedRow < rows.count else { return }
            if case let .object(entry) = rows[table.selectedRow] { onToggle?(entry) }
            table.deselectRow(table.selectedRow)
        }

        @objc func toggleStorageObject(_ sender: NSButton) {
            guard let id = sender.identifier?.rawValue, let entry = storageEntriesByID[id] else { return }
            onToggle?(entry)
        }

        func visibleRowsApproachEnd() { onReachedBottom?() }
    }
}

private protocol SnapshotNativeTableObserver: AnyObject {
    func visibleRowsApproachEnd()
}

private final class SnapshotTableScrollView: NSScrollView {
    private var boundsObserver: NSObjectProtocol?

    deinit {
        if let boundsObserver {
            NotificationCenter.default.removeObserver(boundsObserver)
        }
    }

    func observeVisibleRows(with table: NSTableView, coordinator: SnapshotNativeTableObserver) {
        contentView.postsBoundsChangedNotifications = true
        boundsObserver = NotificationCenter.default.addObserver(
            forName: NSView.boundsDidChangeNotification,
            object: contentView,
            queue: .main
        ) { [weak table, weak coordinator] _ in
            guard let table, let coordinator else { return }
            let visibleRows = table.rows(in: table.visibleRect)
            if visibleRows.location + visibleRows.length >= table.numberOfRows - 4 {
                coordinator.visibleRowsApproachEnd()
            }
        }
    }

    override func layout() {
        super.layout()
        synchronizeTableWidth()
    }

    private func synchronizeTableWidth() {
        guard let table = documentView as? NSTableView,
              let nameColumn = table.tableColumns.first
        else { return }

        let visibleWidth = contentView.bounds.width
        guard visibleWidth > 0 else { return }

        let columnMetrics = table.tableColumns.indices.map { index in
            let column = table.tableColumns[index]
            let cellRect = table.rect(ofColumn: index)
            return (column: column, extraWidth: max(0, cellRect.width - column.width))
        }
        guard let nameMetrics = columnMetrics.first else { return }

        let horizontalInset = table.rect(ofColumn: 0).minX
        let minimumWidth = (horizontalInset * 2) + columnMetrics.reduce(CGFloat.zero) { partial, metrics in
            partial + metrics.column.minWidth + metrics.extraWidth
        }
        let desiredWidth = max(visibleWidth, minimumWidth)
        if abs(table.frame.width - desiredWidth) > 0.5 {
            table.setFrameSize(NSSize(width: desiredWidth, height: table.frame.height))
        }

        let trailingWidth = columnMetrics.dropFirst().reduce(CGFloat.zero) { partial, metrics in
            partial + metrics.column.width + metrics.extraWidth
        }
        let desiredNameWidth = max(
            nameColumn.minWidth,
            desiredWidth - (horizontalInset * 2) - trailingWidth - nameMetrics.extraWidth
        )
        if abs(nameColumn.width - desiredNameWidth) > 0.5 {
            nameColumn.width = desiredNameWidth
        }

        table.layoutSubtreeIfNeeded()
    }
}

private enum SnapshotNativeTable {
    static func scrollView(table: NSTableView) -> SnapshotTableScrollView {
        let scroll = SnapshotTableScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.hasHorizontalScroller = false
        scroll.autohidesScrollers = true
        scroll.drawsBackground = false
        return scroll
    }

    static func scrollView(table: NSTableView, coordinator: SnapshotNativeTableObserver) -> SnapshotTableScrollView {
        let scroll = scrollView(table: table)
        scroll.observeVisibleRows(with: table, coordinator: coordinator)
        return scroll
    }
}

private enum SnapshotNativeRowView {
    static func fileName(
        entry: SnapshotFileEntry,
        leadingInset: CGFloat,
        usesOutlineLayout: Bool
    ) -> NSTableCellView {
        let status = status(for: entry)
        return nameCell(
            name: entry.name,
            icon: icon(for: entry),
            tint: status.tint,
            accessibility: "\(entry.path), \(status.title)",
            leadingInset: leadingInset,
            usesOutlineLayout: usesOutlineLayout
        )
    }

    static func fileChange(entry: SnapshotFileEntry) -> NSTableCellView {
        let status = status(for: entry)
        return textCell(
            text: status.title,
            font: .systemFont(ofSize: 10, weight: .semibold),
            color: status.tint,
            alignment: .left,
            accessibility: "Change: \(status.title)"
        )
    }

    static func fileSize(entry: SnapshotFileEntry) -> NSTableCellView {
        textCell(
            text: formatBytes(Int64(entry.size)),
            font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium),
            color: .secondaryLabelColor,
            alignment: .right,
            accessibility: "Size: \(formatBytes(Int64(entry.size)))"
        )
    }

    static func blockHash(entry: SnapshotBlockEntry) -> NSTableCellView {
        nameCell(
            name: entry.hash,
            icon: "square.stack.3d.up",
            tint: .secondaryLabelColor,
            accessibility: "Block \(entry.hash)",
            leadingInset: 0,
            usesOutlineLayout: false,
            font: .monospacedSystemFont(ofSize: 11, weight: .medium)
        )
    }

    static func blockSize(entry: SnapshotBlockEntry) -> NSTableCellView {
        textCell(
            text: formatBytes(Int64(entry.size)),
            font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium),
            color: .secondaryLabelColor,
            alignment: .right,
            accessibility: "Size: \(formatBytes(Int64(entry.size)))"
        )
    }

    static func blockChangedFiles(entry: SnapshotBlockEntry) -> NSTableCellView {
        textCell(
            text: "\(entry.changedFiles)",
            font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium),
            color: entry.changedFiles > 0 ? .controlAccentColor : .secondaryLabelColor,
            alignment: .right,
            accessibility: "Referenced by \(entry.changedFiles) changed files"
        )
    }

    static func blockReferencedFiles(entry: SnapshotBlockEntry) -> NSTableCellView {
        textCell(
            text: "\(entry.referencingFiles)",
            font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium),
            color: .secondaryLabelColor,
            alignment: .right,
            accessibility: "Referenced by \(entry.referencingFiles) files"
        )
    }

    static func storageObject(
        entry: SnapshotStorageEntry,
        isExpanded: Bool,
        target: AnyObject,
        action: Selector
    ) -> NSTableCellView {
        let cell = NSTableCellView()
        let disclosure = NSButton(title: "", target: target, action: action)
        disclosure.identifier = NSUserInterfaceItemIdentifier(entry.id)
        disclosure.bezelStyle = .disclosure
        disclosure.setButtonType(.pushOnPushOff)
        disclosure.state = isExpanded ? .on : .off
        disclosure.setAccessibilityLabel(isExpanded ? "Collapse storage object \(entry.shortId)" : "Expand storage object \(entry.shortId)")
        disclosure.translatesAutoresizingMaskIntoConstraints = false

        let image = NSImageView(image: NSImage(systemSymbolName: entry.kind == "pack" ? "shippingbox" : "doc", accessibilityDescription: "Storage object") ?? NSImage())
        image.translatesAutoresizingMaskIntoConstraints = false
        image.imageScaling = .scaleProportionallyDown
        image.contentTintColor = entry.kind == "pack" ? .controlAccentColor : .secondaryLabelColor

        let label = NSTextField(labelWithString: entry.shortId)
        label.translatesAutoresizingMaskIntoConstraints = false
        label.font = .monospacedSystemFont(ofSize: 11, weight: .medium)
        label.lineBreakMode = .byTruncatingMiddle

        cell.addSubview(disclosure)
        cell.addSubview(image)
        cell.addSubview(label)
        NSLayoutConstraint.activate([
            disclosure.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 3),
            disclosure.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            disclosure.widthAnchor.constraint(equalToConstant: 16),
            disclosure.heightAnchor.constraint(equalToConstant: 16),
            image.leadingAnchor.constraint(equalTo: disclosure.trailingAnchor, constant: 2),
            image.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            image.widthAnchor.constraint(equalToConstant: 16),
            image.heightAnchor.constraint(equalToConstant: 16),
            label.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 6),
            label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        cell.imageView = image
        cell.textField = label
        cell.setAccessibilityLabel("Storage object \(entry.shortId)")
        return cell
    }

    static func storageKind(entry: SnapshotStorageEntry) -> NSTableCellView {
        textCell(text: entry.kind.capitalized, font: .systemFont(ofSize: 10, weight: .semibold), color: entry.kind == "pack" ? .controlAccentColor : .secondaryLabelColor, alignment: .left, accessibility: "Type: \(entry.kind)")
    }

    static func storageDocument(entry: SnapshotStorageEntry) -> NSTableCellView {
        let text = entry.documentBytes.map { formatBytes(Int64($0)) } ?? "Not recorded"
        return textCell(text: text, font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium), color: entry.documentBytes == nil ? .secondaryLabelColor : .labelColor, alignment: .right, accessibility: "Telegram document size: \(text)")
    }

    static func storageLogical(entry: SnapshotStorageEntry) -> NSTableCellView {
        textCell(text: formatBytes(Int64(entry.logicalBytes)), font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium), color: .secondaryLabelColor, alignment: .right, accessibility: "Logical bytes: \(formatBytes(Int64(entry.logicalBytes)))")
    }

    static func storageReferences(entry: SnapshotStorageEntry) -> NSTableCellView {
        textCell(text: "\(entry.referencedBlocks)", font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium), color: .secondaryLabelColor, alignment: .right, accessibility: "\(entry.referencedBlocks) referenced blocks")
    }

    static func storageRecorded(entry: SnapshotStorageEntry) -> NSTableCellView {
        let rawRecorded = entry.recordedAt
        let recorded = rawRecorded.map { String($0.prefix(16)).replacingOccurrences(of: "T", with: " ") } ?? "Not recorded"
        return textCell(text: recorded, font: .systemFont(ofSize: 10, weight: .medium), color: rawRecorded == nil ? .secondaryLabelColor : .labelColor, alignment: .right, accessibility: "Recorded: \(rawRecorded ?? recorded)")
    }

    static func storageBlock(entry: SnapshotStorageBlockEntry) -> NSTableCellView {
        nameCell(name: entry.hash, icon: "square.stack.3d.up", tint: .secondaryLabelColor, accessibility: "Block slice \(entry.hash)", leadingInset: 22, usesOutlineLayout: false, font: .monospacedSystemFont(ofSize: 10, weight: .medium))
    }

    static func storageSliceType(entry: SnapshotStorageBlockEntry) -> NSTableCellView {
        textCell(text: "Slice", font: .systemFont(ofSize: 10, weight: .medium), color: .secondaryLabelColor, alignment: .left, accessibility: "Physical slice")
    }

    static func storageSliceLength(entry: SnapshotStorageBlockEntry) -> NSTableCellView {
        textCell(text: formatBytes(Int64(entry.length)), font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium), color: .secondaryLabelColor, alignment: .right, accessibility: "Slice length: \(formatBytes(Int64(entry.length)))")
    }

    static func storageBlockSize(entry: SnapshotStorageBlockEntry) -> NSTableCellView {
        textCell(text: formatBytes(Int64(entry.size)), font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium), color: .secondaryLabelColor, alignment: .right, accessibility: "Logical block size: \(formatBytes(Int64(entry.size)))")
    }

    static func storageSliceOffset(entry: SnapshotStorageBlockEntry) -> NSTableCellView {
        textCell(text: "offset \(formatBytes(Int64(entry.offset)))", font: .monospacedDigitSystemFont(ofSize: 10, weight: .medium), color: .secondaryLabelColor, alignment: .right, accessibility: "Slice offset: \(formatBytes(Int64(entry.offset)))")
    }

    static func empty() -> NSTableCellView {
        textCell(text: "", font: .systemFont(ofSize: 10), color: .secondaryLabelColor, alignment: .right, accessibility: "")
    }

    private static func nameCell(
        name: String,
        icon: String,
        tint: NSColor,
        accessibility: String,
        leadingInset: CGFloat,
        usesOutlineLayout: Bool,
        font: NSFont = .systemFont(ofSize: 11, weight: .medium)
    ) -> NSTableCellView {
        let cell = NSTableCellView()
        let image = NSImageView(image: NSImage(systemSymbolName: icon, accessibilityDescription: accessibility) ?? NSImage())
        image.translatesAutoresizingMaskIntoConstraints = false
        image.imageScaling = .scaleProportionallyDown
        image.contentTintColor = tint

        let label = NSTextField(labelWithString: name)
        label.translatesAutoresizingMaskIntoConstraints = false
        label.font = font
        label.lineBreakMode = .byTruncatingMiddle

        cell.addSubview(image)
        cell.addSubview(label)
        NSLayoutConstraint.activate([
            image.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: leadingInset + 4),
            image.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
            image.widthAnchor.constraint(equalToConstant: 16),
            image.heightAnchor.constraint(equalToConstant: 16),
            label.leadingAnchor.constraint(equalTo: image.trailingAnchor, constant: 6),
            label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -6),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        if !usesOutlineLayout {
            cell.imageView = image
            cell.textField = label
        }
        cell.setAccessibilityLabel(accessibility)
        return cell
    }

    private static func textCell(
        text: String,
        font: NSFont,
        color: NSColor,
        alignment: NSTextAlignment,
        accessibility: String
    ) -> NSTableCellView {
        let cell = NSTableCellView()
        let label = NSTextField(labelWithString: text)
        label.translatesAutoresizingMaskIntoConstraints = false
        label.font = font
        label.textColor = color
        label.alignment = alignment
        label.lineBreakMode = .byTruncatingMiddle
        cell.addSubview(label)
        NSLayoutConstraint.activate([
            label.leadingAnchor.constraint(equalTo: cell.leadingAnchor, constant: 4),
            label.trailingAnchor.constraint(equalTo: cell.trailingAnchor, constant: -4),
            label.centerYAnchor.constraint(equalTo: cell.centerYAnchor),
        ])
        cell.textField = label
        cell.setAccessibilityLabel(accessibility)
        return cell
    }

    private static func status(for entry: SnapshotFileEntry) -> (title: String, tint: NSColor) {
        if entry.isAncestorContext { return ("Context", .secondaryLabelColor) }
        switch entry.change {
        case "added": return ("Added", .systemGreen)
        case "deleted": return ("Deleted", .systemRed)
        case "changed": return ("Changed", .systemBlue)
        default: return ("Unchanged", .secondaryLabelColor)
        }
    }

    private static func icon(for entry: SnapshotFileEntry) -> String {
        switch entry.change {
        case "added": return "plus.circle.fill"
        case "deleted": return "minus.circle.fill"
        case "changed": return "pencil.circle.fill"
        default:
            switch entry.kind {
            case "dir": return "folder"
            case "symlink": return "link"
            default: return "doc"
            }
        }
    }
}
