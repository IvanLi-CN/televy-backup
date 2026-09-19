import Foundation

enum SnapshotAccessLaunchdState {
    static func needsRefresh(
        output: String,
        expectedBundleIdentifier: String,
        expectedBundleVersion: String
    ) -> Bool {
        let lines = output.split(whereSeparator: \.isNewline).map(String.init)

        if lines.contains(where: { $0.contains("job state = spawn failed") }) {
            return true
        }

        if lines.contains(where: { line in
            guard let value = value(after: "last exit code = ", in: line),
                  let code = Int(value.split(separator: ":", maxSplits: 1).first ?? "") else {
                return false
            }
            return code != 0
        }) {
            return true
        }

        if let identifier = firstValue(after: "parent bundle identifier = ", in: lines),
           !expectedBundleIdentifier.isEmpty,
           identifier != expectedBundleIdentifier {
            return true
        }
        if let version = firstValue(after: "parent bundle version = ", in: lines),
           !expectedBundleVersion.isEmpty,
           version != expectedBundleVersion {
            return true
        }

        return false
    }

    private static func firstValue(after prefix: String, in lines: [String]) -> String? {
        lines.lazy.compactMap { value(after: prefix, in: $0) }.first
    }

    private static func value(after prefix: String, in line: String) -> String? {
        let trimmed = line.trimmingCharacters(in: .whitespacesAndNewlines)
        guard trimmed.hasPrefix(prefix) else { return nil }
        return trimmed.dropFirst(prefix.count).trimmingCharacters(in: .whitespacesAndNewlines)
    }
}
