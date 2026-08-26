import AppKit
import Combine
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
    let referencingFiles: UInt64

    var id: String { hash }
}

private struct SnapshotBlockPage: Decodable {
    let entries: [SnapshotBlockEntry]
    let nextCursor: String?
}

private struct SnapshotCommandError: Decodable {
    let code: String?
    let message: String?
    let retryable: Bool?
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
    @Published private(set) var filesLoading = false
    @Published private(set) var blocksLoading = false

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
    private var activePresentation: SnapshotInspectionPresentation = .tree
    private var activeChangesOnly = true
    private var activeQuery = ""

    func start(run: RunLogSummary, model: AppModel) {
        self.run = run
        self.model = model
        requestToken += 1
        summary = nil
        issue = nil
        issueRetryable = false
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

    func loadMoreBlocks() {
        guard !blocksReachedEnd, !blocksLoading else { return }
        loadBlockPage()
    }

    var changesAvailable: Bool {
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
        performJSONCommand(model: model, args: ["--json", "snapshots", "inspect", "summary", "--snapshot-id", snapshotId]) {
            (result: Result<SnapshotInspectionSummary, SnapshotRequestFailure>) in
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
        completion: @escaping (Result<SnapshotFilePage, SnapshotRequestFailure>) -> Void
    ) {
        guard let snapshotId = run?.snapshotId, let model else { return }
        var args = [
            "--json", "snapshots", "inspect", "files",
            "--snapshot-id", snapshotId,
            "--presentation", activePresentation.commandValue,
            "--scope", activeChangesOnly ? "changes" : "all",
            "--limit", "200",
        ]
        if let parent { args += ["--parent", parent] }
        if !activeQuery.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            args += ["--query", activeQuery]
        }
        if let cursor { args += ["--cursor", cursor] }
        performJSONCommand(model: model, args: args, completion: completion)
    }

    private func loadBlockPage() {
        guard !blocksLoading, !blocksReachedEnd, let snapshotId = run?.snapshotId, let model else { return }
        blocksLoading = true
        let token = requestToken
        var args = [
            "--json", "snapshots", "inspect", "blocks",
            "--snapshot-id", snapshotId,
            "--limit", "200",
        ]
        if let blockNextCursor { args += ["--cursor", blockNextCursor] }
        performJSONCommand(model: model, args: args) { (result: Result<SnapshotBlockPage, SnapshotRequestFailure>) in
            guard self.requestToken == token else { return }
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

    private func resetPagedContent() {
        listEntries = []
        treeEntries = [:]
        listNextCursor = nil
        listReachedEnd = false
        treeNextCursor = [:]
        treeReachedEnd = []
        treeLoadingParents = []
        blocks = []
        blockNextCursor = nil
        blocksReachedEnd = false
        filesLoading = false
        blocksLoading = false
    }

    private func performJSONCommand<Response: Decodable>(
        model: AppModel,
        args: [String],
        completion: @escaping (Result<Response, SnapshotRequestFailure>) -> Void
    ) {
        guard let cli = model.cliPath() else {
            completion(.failure(.init(message: "TelevyBackup CLI is unavailable.", retryable: false)))
            return
        }
        DispatchQueue.global(qos: .userInitiated).async {
            let result = model.runCommandCapture(exe: cli, args: args, timeoutSeconds: 90)
            let payload = result.status == 0 ? result.stdout : result.stderr
            let decoded: Result<Response, SnapshotRequestFailure>
            if result.status == 0, let data = payload.data(using: .utf8), let response = try? JSONDecoder().decode(Response.self, from: data) {
                decoded = .success(response)
            } else if let data = payload.data(using: .utf8), let error = try? JSONDecoder().decode(SnapshotCommandError.self, from: data) {
                decoded = .failure(.init(
                    message: error.message ?? error.code ?? "Snapshot inspection failed.",
                    retryable: error.retryable ?? false
                ))
            } else {
                decoded = .failure(.init(message: "Snapshot inspection failed.", retryable: false))
            }
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
        let tree = [
            SnapshotFileEntry(path: "Albums", name: "Albums", kind: "dir", change: "unchanged", isAncestorContext: true, size: 0, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: .init(added: 8, deleted: 2, changed: 3)),
            SnapshotFileEntry(path: "Library.photoslibrary", name: "Library.photoslibrary", kind: "dir", change: "changed", isAncestorContext: false, size: 0, mtimeMs: 0, mode: 0, baseline: .init(kind: "dir", size: 0, mtimeMs: 0, mode: 0), descendantChanges: .init(added: 6, deleted: 1, changed: 4)),
        ]
        let albumChildren = [
            SnapshotFileEntry(path: "Albums/2026-08-27.jpg", name: "2026-08-27.jpg", kind: "file", change: "added", isAncestorContext: false, size: 4_120_332, mtimeMs: 0, mode: 0, baseline: nil, descendantChanges: nil),
            SnapshotFileEntry(path: "Albums/old-edit.jpg", name: "old-edit.jpg", kind: "file", change: "deleted", isAncestorContext: false, size: 2_005_120, mtimeMs: 0, mode: 0, baseline: .init(kind: "file", size: 2_005_120, mtimeMs: 0, mode: 0), descendantChanges: nil),
        ]
        treeEntries = ["": tree, "Albums": albumChildren]
        listEntries = tree + albumChildren
        blocks = [
            SnapshotBlockEntry(hash: "9c47a0f53d1a6cb9", size: 1_048_576, referencingFiles: 4),
            SnapshotBlockEntry(hash: "b9d202d17d25e8f1", size: 786_432, referencingFiles: 2),
        ]
        listReachedEnd = true
        treeReachedEnd = ["", "Albums"]
        blocksReachedEnd = true
    }
}

private struct SnapshotRequestFailure: Error {
    let message: String
    let retryable: Bool
}

struct SnapshotRunDetailView: View {
    @Environment(\.appRuntime) private var model
    let run: RunLogSummary
    let onBack: () -> Void

    private enum Tab: String, CaseIterable, Identifiable {
        case summary = "Summary"
        case files = "Files"
        case blocks = "Blocks"

        var id: String { rawValue }
    }

    @StateObject private var store = SnapshotInspectionStore()
    @State private var tab: Tab = {
        let scene = ProcessInfo.processInfo.environment["TELEVYBACKUP_UI_DEMO_SCENE"] ?? ""
        return scene == "main-window-snapshot-changes" || scene == "main-window-snapshot-baseline-unavailable"
            ? .files
            : .summary
    }()
    @State private var presentation: SnapshotInspectionPresentation = .tree
    @State private var changesOnly = true
    @State private var query = ""

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            header
            Picker("", selection: $tab) {
                ForEach(Tab.allCases) { tab in Text(tab.rawValue).tag(tab) }
            }
            .pickerStyle(.segmented)
            .controlSize(.small)
            .fixedSize()
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
        }
        .onChange(of: presentation) { _, _ in reloadFiles() }
        .onChange(of: changesOnly) { _, _ in reloadFiles() }
        .onChange(of: query) { _, _ in reloadFiles() }
    }

    private var header: some View {
        HStack(alignment: .center, spacing: 10) {
            Button(action: onBack) {
                Image(systemName: "chevron.backward")
            }
            .buttonStyle(.borderless)
            .help("Back to history")
            Text("Backup details")
                .font(.system(size: 18, weight: .bold))
            if let status = run.status {
                Text(status.capitalized)
                    .font(.system(size: 11, weight: .semibold))
                    .foregroundStyle(status == "succeeded" ? .green : .secondary)
            }
            Spacer(minLength: 0)
            if let snapshotId = run.snapshotId {
                Text(snapshotId)
                    .font(.system(size: 11, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
            }
        }
    }

    @ViewBuilder
    private var content: some View {
        if store.summaryLoading {
            SnapshotInspectionStateView(icon: "arrow.triangle.2.circlepath", title: "Loading snapshot", detail: "Reading the retained file map.", showsProgress: true)
        } else if let issue = store.issue {
            SnapshotInspectionStateView(icon: "exclamationmark.triangle", title: "Snapshot unavailable", detail: issue, showsProgress: false) {
                if store.issueRetryable {
                    Button("Retry") { store.retry() }
                        .controlSize(.small)
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
            }
        } else {
            SnapshotInspectionStateView(icon: "doc.text", title: "No snapshot data", detail: "This run has no inspectable retained snapshot.", showsProgress: false)
        }
    }

    private func files(summary: SnapshotInspectionSummary) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 10) {
                Picker("Presentation", selection: $presentation) {
                    ForEach(SnapshotInspectionPresentation.allCases) { item in Text(item.rawValue).tag(item) }
                }
                .pickerStyle(.segmented)
                .controlSize(.small)
                .frame(width: 140)
                Toggle("Changes only", isOn: $changesOnly)
                    .toggleStyle(.checkbox)
                    .controlSize(.small)
                    .disabled(!store.changesAvailable)
                TextField("Search paths", text: $query)
                    .textFieldStyle(.roundedBorder)
                    .frame(maxWidth: 240)
                Spacer(minLength: 0)
                if summary.availability.state == "baselineUnavailable" {
                    Text("Direct baseline unavailable")
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                }
            }
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

    private func blocks(summary _: SnapshotInspectionSummary) -> some View {
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
        .onAppear { store.loadBlocksIfNeeded() }
    }

    private func reloadFiles() {
        if store.summary != nil {
            store.configureFiles(presentation: presentation, changesOnly: changesOnly, query: query)
        }
    }
}

private struct SnapshotSummaryView: View {
    let run: RunLogSummary
    let summary: SnapshotInspectionSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Grid(alignment: .leading, horizontalSpacing: 28, verticalSpacing: 8) {
                GridRow { label("Outcome"); Text((run.status ?? "unknown").capitalized) }
                GridRow { label("Duration"); Text(run.durationSeconds.map(formatDuration) ?? "Unavailable") }
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

    private func summaryMetric(_ title: String, _ value: String, _ icon: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Image(systemName: icon).foregroundStyle(.secondary)
            Text(value).font(.system(size: 16, weight: .bold))
            Text(title).font(.system(size: 11, weight: .medium)).foregroundStyle(.secondary)
        }
        .frame(minWidth: 72, alignment: .leading)
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

private final class SnapshotOutlineNode: NSObject {
    let entry: SnapshotFileEntry
    var children: [SnapshotOutlineNode]

    init(entry: SnapshotFileEntry, children: [SnapshotOutlineNode]) {
        self.entry = entry
        self.children = children
    }
}

private struct SnapshotOutlineTable: NSViewRepresentable {
    let entriesByParent: [String: [SnapshotFileEntry]]
    let onExpand: (String) -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let outline = NSOutlineView()
        outline.headerView = nil
        outline.rowSizeStyle = .small
        outline.delegate = context.coordinator
        outline.dataSource = context.coordinator
        let column = NSTableColumn(identifier: NSUserInterfaceItemIdentifier("path"))
        outline.addTableColumn(column)
        outline.outlineTableColumn = column
        let scroll = NSScrollView()
        scroll.documentView = outline
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
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

        func reload() {
            rootNodes = makeNodes(entriesByParent[""] ?? [])
            outline?.reloadData()
        }

        private func makeNodes(_ entries: [SnapshotFileEntry]) -> [SnapshotOutlineNode] {
            entries.map { entry in
                SnapshotOutlineNode(entry: entry, children: makeNodes(entriesByParent[entry.path] ?? []))
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
            if let node = notification.userInfo?["NSObject"] as? SnapshotOutlineNode {
                onExpand?(node.entry.path)
            }
        }

        func outlineView(_: NSOutlineView, viewFor _: NSTableColumn?, item: Any) -> NSView? {
            guard let node = item as? SnapshotOutlineNode else { return nil }
            return SnapshotNativeRowView.file(entry: node.entry)
        }
    }
}

private struct SnapshotFileTable: NSViewRepresentable {
    let entries: [SnapshotFileEntry]
    let onReachedBottom: () -> Void

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let table = NSTableView()
        table.headerView = nil
        table.rowSizeStyle = .small
        table.delegate = context.coordinator
        table.dataSource = context.coordinator
        table.addTableColumn(NSTableColumn(identifier: NSUserInterfaceItemIdentifier("path")))
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

        func tableView(_: NSTableView, viewFor _: NSTableColumn?, row: Int) -> NSView? {
            SnapshotNativeRowView.file(entry: entries[row])
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
        table.headerView = nil
        table.rowSizeStyle = .small
        table.delegate = context.coordinator
        table.dataSource = context.coordinator
        table.addTableColumn(NSTableColumn(identifier: NSUserInterfaceItemIdentifier("block")))
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

        func tableView(_: NSTableView, viewFor _: NSTableColumn?, row: Int) -> NSView? {
            SnapshotNativeRowView.block(entry: entries[row])
        }

        func visibleRowsApproachEnd() { onReachedBottom?() }
    }
}

private protocol SnapshotNativeTableObserver: AnyObject {
    func visibleRowsApproachEnd()
}

private enum SnapshotNativeTable {
    static func scrollView(table: NSTableView, coordinator: SnapshotNativeTableObserver) -> NSScrollView {
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.hasVerticalScroller = true
        scroll.drawsBackground = false
        scroll.contentView.postsBoundsChangedNotifications = true
        NotificationCenter.default.addObserver(
            forName: NSView.boundsDidChangeNotification,
            object: scroll.contentView,
            queue: .main
        ) { [weak table, weak coordinator] _ in
            guard let table, let coordinator else { return }
            let visibleRows = table.rows(in: table.visibleRect)
            if visibleRows.location + visibleRows.length >= table.numberOfRows - 4 {
                coordinator.visibleRowsApproachEnd()
            }
        }
        return scroll
    }
}

private enum SnapshotNativeRowView {
    static func file(entry: SnapshotFileEntry) -> NSTableCellView {
        let state = entry.change == "unchanged" && entry.isAncestorContext ? "context" : entry.change
        let text = "\(entry.name)  \(state)  \(formatBytes(Int64(entry.size)))"
        return make(text: text, icon: icon(for: entry.change), accessibility: "\(entry.path), \(state)")
    }

    static func block(entry: SnapshotBlockEntry) -> NSTableCellView {
        make(
            text: "\(entry.hash)  \(formatBytes(Int64(entry.size)))  \(entry.referencingFiles) files",
            icon: "square.stack.3d.up",
            accessibility: "Block \(entry.hash), \(entry.referencingFiles) referencing files"
        )
    }

    private static func make(text: String, icon: String, accessibility: String) -> NSTableCellView {
        let cell = NSTableCellView()
        let image = NSImageView(image: NSImage(systemSymbolName: icon, accessibilityDescription: accessibility) ?? NSImage())
        image.frame = NSRect(x: 4, y: 2, width: 16, height: 16)
        image.imageScaling = .scaleProportionallyDown
        let label = NSTextField(labelWithString: text)
        label.font = .systemFont(ofSize: 11, weight: .medium)
        label.lineBreakMode = .byTruncatingMiddle
        label.frame = NSRect(x: 26, y: 1, width: 400, height: 18)
        label.autoresizingMask = [.width]
        label.setAccessibilityLabel(accessibility)
        cell.addSubview(image)
        cell.addSubview(label)
        cell.textField = label
        cell.setAccessibilityLabel(accessibility)
        return cell
    }

    private static func icon(for change: String) -> String {
        switch change {
        case "added": return "plus.circle.fill"
        case "deleted": return "minus.circle.fill"
        case "changed": return "pencil.circle.fill"
        default: return "folder"
        }
    }
}
