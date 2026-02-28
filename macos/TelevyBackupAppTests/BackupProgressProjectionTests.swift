import Foundation

@discardableResult
private func expect(_ ok: @autoclosure () -> Bool, _ message: String) -> Bool {
    if !ok() {
        fputs("FAIL: \(message)\n", stderr)
        exit(1)
    }
    return true
}

private func expectClose(_ lhs: Double, _ rhs: Double, _ message: String, eps: Double = 0.0001) {
    expect(abs(lhs - rhs) <= eps, "\(message) (got=\(lhs) expected=\(rhs))")
}

private func expectOrdered(_ f: BackupProgressFractions, _ message: String) {
    expect(f.needUploadConfirmed >= 0.0, "\(message): need < 0")
    expect(f.uploadingCurrent >= f.needUploadConfirmed, "\(message): current < need")
    expect(f.backedUp >= f.uploadingCurrent, "\(message): backed < current")
    expect(f.scanned >= f.backedUp, "\(message): scanned < backed")
    expect(f.scanned <= 1.0, "\(message): scanned > 1")
}

private func mk(
    phase: String,
    sourceBytesTotal: Int64,
    sourceFilesTotal: Int64? = nil,
    filesTotal: Int64? = nil,
    filesDone: Int64? = nil,
    bytesRead: Int64? = nil,
    bytesDeduped: Int64? = nil,
    sourceBytesNeedUploadTotal: Int64? = nil,
    uploadBytesTotal: Int64? = nil,
    bytesUploadedSource: Int64? = nil,
    bytesUploaded: Int64? = nil,
    bytesUploadedConfirmed: Int64? = nil
) -> StatusProgress {
    StatusProgress(
        phase: phase,
        sourceFilesTotal: sourceFilesTotal,
        sourceBytesTotal: sourceBytesTotal,
        sourceBytesNeedUploadTotal: sourceBytesNeedUploadTotal,
        filesTotal: filesTotal,
        filesDone: filesDone,
        chunksTotal: nil,
        chunksDone: nil,
        bytesRead: bytesRead,
        uploadBytesTotal: uploadBytesTotal,
        bytesUploadedConfirmed: bytesUploadedConfirmed,
        bytesUploadedSource: bytesUploadedSource,
        bytesUploaded: bytesUploaded,
        bytesDownloaded: nil,
        bytesDeduped: bytesDeduped
    )
}

private func test_nil_without_source_total() {
    let p = mk(phase: "scan", sourceBytesTotal: 0)
    expect(BackupProgressProjection.compute(p) == nil, "sourceBytesTotal<=0 should produce nil")
}

private func test_scan_dedup_only_does_not_fake_upload() {
    let p = mk(
        phase: "scan",
        sourceBytesTotal: 1_000,
        sourceFilesTotal: 1_000,
        filesDone: 140,
        bytesRead: 0,
        bytesDeduped: 30,
        sourceBytesNeedUploadTotal: 0,
        uploadBytesTotal: 0,
        bytesUploadedSource: 0,
        bytesUploaded: 0,
        bytesUploadedConfirmed: 0
    )
    guard let f = BackupProgressProjection.compute(p) else {
        expect(false, "scan dedup-only should compute")
        return
    }
    expectClose(f.scanned, 0.14, "scan should follow files progress")
    expectClose(f.backedUp, 0.03, "backed-up should follow dedup bytes")
    expectClose(f.needUploadConfirmed, 0.0, "scan should not fake confirmed upload")
    expectClose(f.uploadingCurrent, 0.0, "scan should not fake in-flight upload")
    expectOrdered(f, "scan dedup-only ordering")
}

private func test_scan_complete_reaches_100_percent() {
    let p = mk(
        phase: "scan",
        sourceBytesTotal: 1_000,
        sourceFilesTotal: 100,
        filesDone: 100,
        bytesRead: 250,
        bytesDeduped: 250
    )
    guard let f = BackupProgressProjection.compute(p) else {
        expect(false, "scan complete should compute")
        return
    }
    expectClose(f.scanned, 1.0, "scan completion should be 100%")
    expect(f.backedUp < 1.0, "running backed-up should stay <100%")
    expectOrdered(f, "scan complete ordering")
}

private func test_upload_uses_current_when_confirmed_lags() {
    let p = mk(
        phase: "upload",
        sourceBytesTotal: 1_000,
        sourceFilesTotal: 100,
        filesDone: 100,
        bytesRead: 100,
        bytesDeduped: 200,
        sourceBytesNeedUploadTotal: 600,
        uploadBytesTotal: 600,
        bytesUploadedSource: 120,
        bytesUploaded: 180,
        bytesUploadedConfirmed: 0
    )
    guard let f = BackupProgressProjection.compute(p) else {
        expect(false, "upload progress should compute")
        return
    }
    // backedUp=(200+120)/1000=0.32
    expectClose(f.backedUp, 0.32, "backed-up uses dedup+uploaded-source")
    expect(f.needUploadConfirmed > 0.0, "confirmed layer should still move with current fallback")
    expect(f.uploadingCurrent >= f.needUploadConfirmed, "current >= confirmed")
    expectOrdered(f, "upload lag ordering")
}

private func test_upload_zero_denominator_stays_zero() {
    let p = mk(
        phase: "upload",
        sourceBytesTotal: 1_000,
        sourceFilesTotal: 100,
        filesDone: 100,
        bytesRead: 100,
        bytesDeduped: 400,
        sourceBytesNeedUploadTotal: 0,
        uploadBytesTotal: 0,
        bytesUploadedSource: 0,
        bytesUploaded: 0,
        bytesUploadedConfirmed: 0
    )
    guard let f = BackupProgressProjection.compute(p) else {
        expect(false, "upload zero denominator should compute")
        return
    }
    expectClose(f.needUploadConfirmed, 0.0, "upload zero denominator should not fake progress")
    expectClose(f.uploadingCurrent, 0.0, "upload current should not fake progress")
    expectOrdered(f, "upload zero denominator ordering")
}

private func test_index_zero_denominator_with_uploaded_bytes_stays_zero() {
    let p = mk(
        phase: "index",
        sourceBytesTotal: 1_000,
        sourceFilesTotal: 100,
        filesDone: 100,
        bytesRead: 100,
        bytesDeduped: 700,
        sourceBytesNeedUploadTotal: 0,
        uploadBytesTotal: 0,
        bytesUploadedSource: 80,
        bytesUploaded: 120,
        bytesUploadedConfirmed: 40
    )
    guard let f = BackupProgressProjection.compute(p) else {
        expect(false, "index zero denominator should compute")
        return
    }
    expectClose(f.needUploadConfirmed, 0.0, "index zero denominator should not become 99%")
    expectClose(f.uploadingCurrent, 0.0, "index zero denominator current should stay zero")
    expect(f.backedUp > 0.0, "backed-up should still move")
    expectOrdered(f, "index zero denominator ordering")
}

private func test_invalid_files_done_does_not_break_ordering() {
    let p = mk(
        phase: "scan",
        sourceBytesTotal: 10_000,
        sourceFilesTotal: 100,
        filesDone: 999,
        bytesRead: 500,
        bytesDeduped: 700,
        sourceBytesNeedUploadTotal: 200,
        uploadBytesTotal: 200
    )
    guard let f = BackupProgressProjection.compute(p) else {
        expect(false, "invalid file counts should still compute")
        return
    }
    expect(f.scanned >= 0.07, "scanned should stay driven by valid dimensions")
    expectOrdered(f, "invalid files ordering")
}

private func test_scan_files_done_over_total_clamps_to_complete() {
    let p = mk(
        phase: "scan",
        sourceBytesTotal: 10_000,
        sourceFilesTotal: 100,
        filesDone: 101,
        bytesRead: 50,
        bytesDeduped: 50
    )
    guard let f = BackupProgressProjection.compute(p) else {
        expect(false, "filesDone overflow should still compute")
        return
    }
    expectClose(f.scanned, 1.0, "filesDone overflow should clamp scan layer to completion")
    expectOrdered(f, "filesDone overflow ordering")
}

private func test_projection_invariants_across_matrix() {
    let phases = ["scan", "upload", "index"]
    for phase in phases {
        for filesDone in [0, 50, 100] {
            for dedup in [0, 150, 400] {
                for read in [0, 200, 600] {
                    for uploaded in [0, 50, 180] {
                        let p = mk(
                            phase: phase,
                            sourceBytesTotal: 1_000,
                            sourceFilesTotal: 100,
                            filesDone: Int64(filesDone),
                            bytesRead: Int64(read),
                            bytesDeduped: Int64(dedup),
                            sourceBytesNeedUploadTotal: 600,
                            uploadBytesTotal: 600,
                            bytesUploadedSource: Int64(min(uploaded, 300)),
                            bytesUploaded: Int64(uploaded),
                            bytesUploadedConfirmed: Int64(max(0, uploaded - 30))
                        )
                        guard let f = BackupProgressProjection.compute(p) else {
                            expect(false, "projection matrix should always compute")
                            return
                        }
                        expectOrdered(f, "matrix phase=\(phase) filesDone=\(filesDone) dedup=\(dedup) read=\(read) uploaded=\(uploaded)")
                    }
                }
            }
        }
    }
}

private func test_display_need_upload_bytes_prefers_upload_total() {
    let p = mk(
        phase: "upload",
        sourceBytesTotal: 1_000,
        sourceBytesNeedUploadTotal: 321,
        uploadBytesTotal: 999
    )
    let bytes = BackupProgressProjection.displayNeedUploadBytes(p)
    expect(bytes == 999, "need-upload display should prefer task uploadBytesTotal")
}

private func test_display_need_upload_bytes_uses_larger_total() {
    let p = mk(
        phase: "upload",
        sourceBytesTotal: 1_000,
        sourceBytesNeedUploadTotal: 600,
        uploadBytesTotal: 500
    )
    let bytes = BackupProgressProjection.displayNeedUploadBytes(p)
    expect(bytes == 600, "need-upload display should not regress when source total is larger")
}

private func test_display_need_upload_bytes_falls_back_to_upload_total() {
    let p = mk(
        phase: "upload",
        sourceBytesTotal: 1_000,
        sourceBytesNeedUploadTotal: nil,
        uploadBytesTotal: 456
    )
    let bytes = BackupProgressProjection.displayNeedUploadBytes(p)
    expect(bytes == 456, "need-upload display should fallback to uploadBytesTotal")
}

private func test_display_uploaded_bytes_prefers_current() {
    let p = mk(
        phase: "upload",
        sourceBytesTotal: 1_000,
        bytesUploadedSource: 120,
        bytesUploaded: 180,
        bytesUploadedConfirmed: 150
    )
    let bytes = BackupProgressProjection.displayUploadedBytes(p)
    expect(bytes == 180, "uploaded display should prefer current task uploaded bytes")
}

private func test_display_uploaded_bytes_fallback_chain() {
    var p = mk(
        phase: "upload",
        sourceBytesTotal: 1_000,
        bytesUploadedSource: 120,
        bytesUploaded: nil,
        bytesUploadedConfirmed: 150
    )
    expect(BackupProgressProjection.displayUploadedBytes(p) == 150, "uploaded display should fallback to confirmed bytes")
    p.bytesUploadedConfirmed = nil
    expect(BackupProgressProjection.displayUploadedBytes(p) == 120, "uploaded display should fallback to source bytes")
}

private func test_display_need_upload_bytes_clamps_negative() {
    var p = mk(
        phase: "upload",
        sourceBytesTotal: 1_000,
        sourceBytesNeedUploadTotal: -10,
        uploadBytesTotal: -20
    )
    expect(BackupProgressProjection.displayNeedUploadBytes(p) == 0, "negative source need should clamp to 0")
    p.sourceBytesNeedUploadTotal = nil
    expect(BackupProgressProjection.displayNeedUploadBytes(p) == 0, "negative upload total should clamp to 0")
}

private func test_display_remaining_upload_bytes_clamps_at_zero() {
    let p = mk(
        phase: "scan_upload",
        sourceBytesTotal: 1_000,
        sourceBytesNeedUploadTotal: 200,
        uploadBytesTotal: 220,
        bytesUploaded: 300
    )
    expect(
        BackupProgressProjection.displayRemainingUploadBytes(p) == 0,
        "remaining upload bytes should clamp to zero when uploaded exceeds discovered need"
    )
}

private func test_display_remaining_upload_bytes_tracks_discovered_gap() {
    let p = mk(
        phase: "scan_upload",
        sourceBytesTotal: 1_000,
        sourceBytesNeedUploadTotal: 600,
        uploadBytesTotal: 700,
        bytesUploaded: 120
    )
    expect(
        BackupProgressProjection.displayRemainingUploadBytes(p) == 580,
        "remaining upload bytes should be discovered need minus uploaded"
    )
}

private func test_need_upload_scope_by_phase() {
    var p = mk(phase: "scan_upload", sourceBytesTotal: 1_000)
    expect(BackupProgressProjection.needUploadScope(p) == .discovered, "scan_upload should use discovered scope")
    p.phase = "upload"
    expect(BackupProgressProjection.needUploadScope(p) == .final, "upload should use final scope")
    p.phase = "index"
    expect(BackupProgressProjection.needUploadScope(p) == .final, "index should use final scope")
}

private func runAllTests() {
    test_nil_without_source_total()
    test_scan_dedup_only_does_not_fake_upload()
    test_scan_complete_reaches_100_percent()
    test_upload_uses_current_when_confirmed_lags()
    test_upload_zero_denominator_stays_zero()
    test_index_zero_denominator_with_uploaded_bytes_stays_zero()
    test_invalid_files_done_does_not_break_ordering()
    test_scan_files_done_over_total_clamps_to_complete()
    test_projection_invariants_across_matrix()
    test_display_need_upload_bytes_prefers_upload_total()
    test_display_need_upload_bytes_uses_larger_total()
    test_display_need_upload_bytes_falls_back_to_upload_total()
    test_display_uploaded_bytes_prefers_current()
    test_display_uploaded_bytes_fallback_chain()
    test_display_need_upload_bytes_clamps_negative()
    test_display_remaining_upload_bytes_clamps_at_zero()
    test_display_remaining_upload_bytes_tracks_discovered_gap()
    test_need_upload_scope_by_phase()
}

@main
enum BackupProgressProjectionTestsMain {
    static func main() {
        runAllTests()
        print("OK: BackupProgressProjectionTests")
    }
}
