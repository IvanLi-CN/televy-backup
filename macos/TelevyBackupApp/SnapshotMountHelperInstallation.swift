import Foundation

enum SnapshotMountHelperInstallation {
    static func perform<Status>(
        install: (@escaping (Bool, String?) -> Void) -> Void,
        refresh: @escaping (@escaping (Status?, String?) -> Void) -> Void,
        isReady: @escaping (Status) -> Bool,
        completion: @escaping (Bool, String?, Status?) -> Void
    ) {
        install { installed, installationError in
            refresh { status, refreshError in
                let ready = status.map(isReady) ?? false
                // Preserve the existing refresh-overwrites-installation behavior for
                // the failing regression; the repair follows the CI reproduction.
                _ = installationError
                completion(installed && ready, refreshError, status)
            }
        }
    }
}
