import Foundation

enum MenuBarActivityState: String, Equatable {
    case idle
    case failure
    case backup
    case restore
    case verify
    case bidirectional

    var accessibilityDescription: String {
        switch self {
        case .idle: return "Idle"
        case .failure: return "Error"
        case .backup: return "Backing up"
        case .restore: return "Restoring"
        case .verify: return "Verifying"
        case .bidirectional: return "Bidirectional sync"
        }
    }
}
