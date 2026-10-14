import Foundation

enum SnapshotMountHelperInstallation {
    static func perform<Status>(
        install: (@escaping (Bool, String?) -> Void) -> Void,
        refresh: @escaping (@escaping (Status?, String?) -> Void) -> Void,
        isReady: @escaping (Status) -> Bool,
        readinessError: @escaping (Status) -> String? = { _ in nil },
        completion: @escaping (Bool, String?, Status?) -> Void
    ) {
        install { installed, installationError in
            refresh { status, refreshError in
                let ready = status.map(isReady) ?? false
                if !installed {
                    completion(false, installationError ?? "The mount helper could not be installed", status)
                } else if !ready {
                    let reason = refreshError ?? status.flatMap(readinessError)
                    let message = "Installation completed, but the communication check failed."
                    completion(false, reason.map { message + " " + $0 } ?? message, status)
                } else {
                    completion(true, nil, status)
                }
            }
        }
    }
}
