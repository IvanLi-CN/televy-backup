import AppKit
import Foundation
import SwiftUI

/// Render the production failure notice and history rows from fixed synthetic data.
@main
enum BackupFailureHistoryPreview {
    @MainActor
    static func main() throws {
        guard CommandLine.arguments.count == 2 else { exit(2) }
        let output = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        let failed = run(id: "fixture-failed", status: "failed", code: "snapshot.acquire_failed", reason: "protocol_mismatch")
        let interrupted = run(id: "fixture-interrupted", status: "interrupted", code: "task.interrupted", reason: nil)
        for (name, scheme) in [("light", ColorScheme.light), ("dark", ColorScheme.dark)] {
            let content = VStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text("Photos").font(.system(size: 18, weight: .bold))
                    Text("Failed").font(.system(size: 12, weight: .semibold)).foregroundStyle(.red)
                }
                BackupFailureNotice(
                    issue: BackupFailurePresentation(code: failed.errorCode, reason: failed.errorReason),
                    code: failed.errorCode,
                    openSettings: {},
                    openDiagnostics: {}
                )
                Divider()
                Text("History").font(.system(size: 12, weight: .semibold)).foregroundStyle(.secondary)
                RunLogRow(run: failed)
                Divider()
                RunLogRow(run: interrupted)
            }
            .padding(24)
            .frame(width: 880, alignment: .leading)
            .background(scheme == .dark ? Color(red: 0.16, green: 0.17, blue: 0.19) : Color.white)
            .environment(\.colorScheme, scheme)
            .environment(\.locale, Locale(identifier: "en_US"))
            .environment(\.timeZone, TimeZone(secondsFromGMT: 0)!)
            let renderer = ImageRenderer(content: content)
            renderer.scale = 2
            guard let image = renderer.nsImage,
                  let tiff = image.tiffRepresentation,
                  let bitmap = NSBitmapImageRep(data: tiff),
                  let png = bitmap.representation(using: .png, properties: [:])
            else { exit(1) }
            try png.write(to: output.appendingPathComponent("backup-failure-\(name).png"))

            let serviceFailures = [
                run(id: "fixture-telegram", status: "failed", code: "telegram.unavailable", reason: nil),
                run(id: "fixture-mount", status: "failed", code: "snapshot.acquire_failed", reason: "mount_permission_denied"),
            ]
            let serviceContent = VStack(alignment: .leading, spacing: 20) {
                Text("Backup failure details").font(.system(size: 18, weight: .bold))
                ForEach(serviceFailures, id: \.id) { failure in
                    VStack(alignment: .leading, spacing: 12) {
                        Divider()
                        BackupFailureNotice(
                            issue: BackupFailurePresentation(code: failure.errorCode, reason: failure.errorReason),
                            code: failure.errorCode, openSettings: {}, openDiagnostics: {}
                        )
                        RunLogRow(run: failure)
                    }
                }
            }
            .padding(24)
            .frame(width: 680, alignment: .leading)
            .background(scheme == .dark ? Color(red: 0.16, green: 0.17, blue: 0.19) : Color.white)
            .environment(\.colorScheme, scheme)
            .environment(\.locale, Locale(identifier: "en_US"))
            .environment(\.timeZone, TimeZone(secondsFromGMT: 0)!)
            let serviceRenderer = ImageRenderer(content: serviceContent)
            serviceRenderer.scale = 2
            guard let serviceImage = serviceRenderer.nsImage,
                  let serviceTiff = serviceImage.tiffRepresentation,
                  let serviceBitmap = NSBitmapImageRep(data: serviceTiff),
                  let servicePNG = serviceBitmap.representation(using: .png, properties: [:])
            else { exit(1) }
            try servicePNG.write(to: output.appendingPathComponent("backup-service-errors-\(name).png"))

            var installationError: String?
            SnapshotMountHelperInstallation.perform(
                install: { $0(false, "Mount helper installation failed: bootstrap failed (fixture)") },
                refresh: { $0(false, nil) },
                isReady: { $0 },
                completion: { _, message, _ in installationError = message }
            )
            guard let installationError else { exit(1) }
            let errorContent = VStack(alignment: .leading, spacing: 14) {
                Text("Settings → Snapshots").font(.system(size: 18, weight: .bold))
                SnapshotSettingsErrorNotice(message: installationError)
            }
            .padding(24)
            .frame(width: 750, alignment: .leading)
            .background(scheme == .dark ? Color(red: 0.16, green: 0.17, blue: 0.19) : Color.white)
            .environment(\.colorScheme, scheme)
            let errorRenderer = ImageRenderer(content: errorContent)
            errorRenderer.scale = 2
            guard let errorImage = errorRenderer.nsImage,
                  let errorTiff = errorImage.tiffRepresentation,
                  let errorBitmap = NSBitmapImageRep(data: errorTiff),
                  let errorPNG = errorBitmap.representation(using: .png, properties: [:])
            else { exit(1) }
            try errorPNG.write(to: output.appendingPathComponent("mount-helper-install-error-\(name).png"))

            let candidate = String(repeating: "a", count: 40)
            let helperContent = VStack(alignment: .leading, spacing: 18) {
                Text("Settings → Snapshots").font(.system(size: 18, weight: .bold))
                Text("Helper update states").font(.system(size: 12)).foregroundStyle(.secondary)
                ForEach(["outdated", "current", "missing", "pending"], id: \.self) { state in
                    // Native GroupBox's AppKit backing is omitted by ImageRenderer.
                    // Keep this source-owned preview container entirely SwiftUI.
                    VStack(alignment: .leading, spacing: 12) {
                        Divider()
                        SnapshotMountHelperStatusView(
                            readiness: SnapshotMountHelperReadiness(
                                reachable: state != "missing", daemonReady: state == "current",
                                buildCommit: state == "current" ? candidate : nil,
                                expectedBuildCommit: candidate,
                                activeMounts: state == "pending" ? 1 : 0,
                                liveMounts: 0,
                                pendingCleanup: state == "pending" ? 1 : 0,
                                recoverableStaleMounts: state == "pending" ? 1 : 0,
                                unknownBlockingMounts: 0
                            ),
                            path: "/Library/PrivilegedHelperTools/com.example.snapshot-mount-helper",
                            version: state == "missing" ? nil : "0.5.1",
                            error: state == "missing" ? "The mount helper is not installed." : nil,
                            activeMounts: state == "pending" ? 1 : 0,
                            pendingCleanup: state == "pending" ? 1 : 0,
                            liveMounts: state == "pending" ? 0 : 0,
                            recoverableStaleMounts: state == "pending" ? 1 : 0,
                            unknownBlockingMounts: 0,
                            busy: false, install: {}, recover: {}
                        )
                    }
                }
                Text("Installing or updating requires administrator approval.")
                    .font(.system(size: 11)).foregroundStyle(.secondary)
            }
            .padding(24)
            .frame(width: 430, alignment: .leading)
            .background(scheme == .dark ? Color(red: 0.16, green: 0.17, blue: 0.19) : Color.white)
            .environment(\.colorScheme, scheme)
            let helperRenderer = ImageRenderer(content: helperContent)
            helperRenderer.scale = 2
            guard let helperImage = helperRenderer.nsImage,
                  let helperTiff = helperImage.tiffRepresentation,
                  let helperBitmap = NSBitmapImageRep(data: helperTiff),
                  let helperPNG = helperBitmap.representation(using: .png, properties: [:])
            else { exit(1) }
            try helperPNG.write(to: output.appendingPathComponent("mount-helper-update-\(name).png"))

            let activeHelperContent = VStack(alignment: .leading, spacing: 12) {
                Text("Settings → Snapshots").font(.system(size: 18, weight: .bold))
                SnapshotMountHelperStatusView(
                    readiness: SnapshotMountHelperReadiness(
                        reachable: true, daemonReady: false,
                        buildCommit: nil, expectedBuildCommit: String(repeating: "a", count: 40)
                    ),
                    path: "/Library/PrivilegedHelperTools/com.example.snapshot-mount-helper",
                    version: "0.5.0",
                    error: nil,
                    activeMounts: 1,
                    pendingCleanup: 0,
                    liveMounts: nil,
                    recoverableStaleMounts: nil,
                    unknownBlockingMounts: nil,
                    busy: false,
                    install: {},
                    recover: {}
                )
            }
            .padding(24)
            .frame(width: 430, alignment: .leading)
            .background(scheme == .dark ? Color(red: 0.16, green: 0.17, blue: 0.19) : Color.white)
            .environment(\.colorScheme, scheme)
            let activeHelperRenderer = ImageRenderer(content: activeHelperContent)
            activeHelperRenderer.scale = 2
            guard let activeImage = activeHelperRenderer.nsImage,
                  let activeTiff = activeImage.tiffRepresentation,
                  let activeBitmap = NSBitmapImageRep(data: activeTiff),
                  let activePNG = activeBitmap.representation(using: .png, properties: [:])
            else { exit(1) }
            try activePNG.write(to: output.appendingPathComponent("mount-helper-active-\(name).png"))
        }
    }

    private static func run(id: String, status: String, code: String, reason: String?) -> RunLogSummary {
        RunLogSummary(
            id: id, runId: id, kind: "backup", targetId: "fixture-target", endpointId: "fixture-endpoint",
            sourcePath: nil, snapshotId: nil, status: status, errorCode: code, durationSeconds: 23,
            startedAt: Date(timeIntervalSince1970: 1_767_225_600),
            finishedAt: status == "failed" ? Date(timeIntervalSince1970: 1_767_225_623) : nil,
            logURL: URL(fileURLWithPath: "/tmp/fixture-run.ndjson"),
            bytesUploaded: nil, bytesDeduped: nil, bytesWritten: nil, bytesChecked: nil,
            filesRestored: nil, chunksDownloaded: nil, chunksChecked: nil,
            ignoreRuleFiles: nil, ignoreInvalidRules: nil, errorReason: reason
        )
    }
}
