import Foundation

struct BackupProgressFractions: Equatable {
    let scanned: Double
    let backedUp: Double
    let needUploadConfirmed: Double
    let uploadingCurrent: Double
}

enum NeedUploadScope: Equatable {
    case discovered
    case final
}

enum BackupProgressProjection {
    private static let runningPhases: Set<String> = ["scan", "scan_upload", "upload", "index"]
    private static let finalNeedUploadPhases: Set<String> = ["upload", "index"]

    private static func normalizedPhase(_ progress: StatusProgress?) -> String {
        guard let phase = progress?.phase else { return "" }
        return phase.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
    }

    static func needUploadScope(_ progress: StatusProgress?) -> NeedUploadScope {
        finalNeedUploadPhases.contains(normalizedPhase(progress)) ? .final : .discovered
    }

    static func displayUploadedBytes(_ progress: StatusProgress?) -> Int64? {
        guard let progress else { return nil }
        if let uploadedCurrent = progress.bytesUploaded {
            return max(0, uploadedCurrent)
        }
        if let uploadedConfirmed = progress.bytesUploadedConfirmed {
            return max(0, uploadedConfirmed)
        }
        if let uploadedSource = progress.bytesUploadedSource {
            return max(0, uploadedSource)
        }
        return nil
    }

    static func displayNeedUploadBytes(_ progress: StatusProgress?) -> Int64? {
        guard let progress else { return nil }
        if let uploadTotal = progress.uploadBytesTotal {
            let sourceTotal = max(0, progress.sourceBytesNeedUploadTotal ?? 0)
            return max(max(0, uploadTotal), sourceTotal)
        }
        if let source = progress.sourceBytesNeedUploadTotal {
            return max(0, source)
        }
        return nil
    }

    static func displayRemainingUploadBytes(_ progress: StatusProgress?) -> Int64? {
        guard let needUpload = displayNeedUploadBytes(progress) else { return nil }
        let uploaded = max(0, displayUploadedBytes(progress) ?? 0)
        return max(0, needUpload - uploaded)
    }

    static func compute(_ progress: StatusProgress?) -> BackupProgressFractions? {
        guard let progress,
              let totalBytes = progress.sourceBytesTotal,
              totalBytes > 0
        else {
            return nil
        }

        let phase = progress.phase.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        let inRunningPhase = runningPhases.contains(phase)
        let runtimeCap = inRunningPhase ? 0.999 : 1.0
        let total = Double(totalBytes)

        let uploadedSource = max(Int64(0), progress.bytesUploadedSource ?? progress.bytesUploaded ?? 0)
        let uploadedCurrent = max(Int64(0), progress.bytesUploaded ?? uploadedSource)
        let uploadedConfirmed = max(Int64(0), progress.bytesUploadedConfirmed ?? uploadedSource)
        let deduped = max(Int64(0), progress.bytesDeduped ?? 0)
        let read = max(Int64(0), progress.bytesRead ?? 0)

        let successBytes = uploadedSource > (Int64.max - deduped) ? Int64.max : (uploadedSource + deduped)
        let backedUp = min(runtimeCap, max(0.0, Double(successBytes) / total))

        let scanRead = min(1.0, max(0.0, Double(read) / total))
        let scanFiles: Double = {
            let done = max(Int64(0), progress.filesDone ?? 0)
            if let sourceFilesTotal = progress.sourceFilesTotal, sourceFilesTotal > 0 {
                let clampedDone = min(done, sourceFilesTotal)
                return min(1.0, max(0.0, Double(clampedDone) / Double(sourceFilesTotal)))
            }
            if let filesTotal = progress.filesTotal, filesTotal > 0 {
                let clampedDone = min(done, filesTotal)
                return min(1.0, max(0.0, Double(clampedDone) / Double(filesTotal)))
            }
            return 0.0
        }()
        let scanned = min(1.0, max(max(scanRead, scanFiles), backedUp))

        let uploadWorkloadTotal = max(Int64(0), progress.uploadBytesTotal ?? 0)
        let sourceNeedUploadTotal = max(Int64(0), progress.sourceBytesNeedUploadTotal ?? 0)
        let discoveredNeedTotal = max(uploadWorkloadTotal, sourceNeedUploadTotal)
        let denominator = discoveredNeedTotal

        let needUploadRaw: Double
        let uploadingRaw: Double
        if denominator > 0 {
            let denom = Double(denominator)
            let needBase = uploadedConfirmed > 0 ? uploadedConfirmed : uploadedCurrent
            needUploadRaw = max(0.0, Double(min(needBase, denominator)) / denom)
            uploadingRaw = max(needUploadRaw, Double(min(uploadedCurrent, denominator)) / denom)
        } else {
            needUploadRaw = 0.0
            uploadingRaw = 0.0
        }

        let needUploadConfirmed = min(backedUp, min(runtimeCap, needUploadRaw))
        let uploadingCurrent = min(backedUp, min(runtimeCap, max(needUploadConfirmed, uploadingRaw)))

        return BackupProgressFractions(
            scanned: scanned,
            backedUp: backedUp,
            needUploadConfirmed: needUploadConfirmed,
            uploadingCurrent: uploadingCurrent
        )
    }
}
