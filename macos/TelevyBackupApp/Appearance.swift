import AppKit
import SwiftUI

enum AppAppearanceOverride: String, CaseIterable {
    case system
    case light
    case dark

    static let environmentKey = "TELEVYBACKUP_UI_APPEARANCE"

    static func fromEnvironment(_ env: [String: String] = ProcessInfo.processInfo.environment) -> AppAppearanceOverride {
        guard let raw = env[environmentKey]?
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased(),
            !raw.isEmpty
        else {
            return .system
        }
        return AppAppearanceOverride(rawValue: raw) ?? .system
    }

    var nsAppearance: NSAppearance? {
        switch self {
        case .system:
            return nil
        case .light:
            return NSAppearance(named: .aqua)
        case .dark:
            return NSAppearance(named: .darkAqua)
        }
    }

    var colorScheme: ColorScheme? {
        switch self {
        case .system:
            return nil
        case .light:
            return .light
        case .dark:
            return .dark
        }
    }

    func apply(to app: NSApplication) {
        if let nsAppearance {
            if app.appearance?.name != nsAppearance.name {
                app.appearance = nsAppearance
            }
        } else if app.appearance != nil {
            app.appearance = nil
        }
    }

    func apply(to window: NSWindow) {
        if let nsAppearance {
            if window.appearance?.name != nsAppearance.name {
                window.appearance = nsAppearance
            }
        } else if window.appearance != nil {
            window.appearance = nil
        }
    }

    func apply(to popover: NSPopover) {
        if let nsAppearance {
            if popover.appearance?.name != nsAppearance.name {
                popover.appearance = nsAppearance
            }
        } else if popover.appearance != nil {
            popover.appearance = nil
        }
    }
}

extension View {
    @ViewBuilder
    func appAppearanceOverride(_ appAppearance: AppAppearanceOverride) -> some View {
        if let colorScheme = appAppearance.colorScheme {
            self.preferredColorScheme(colorScheme)
        } else {
            self
        }
    }
}

struct PopoverTheme {
    let colorScheme: ColorScheme

    private var isDark: Bool { colorScheme == .dark }

    var cardBackground: Color {
        isDark ? Color.white.opacity(0.08) : Color.white.opacity(0.18)
    }

    var cardStroke: Color {
        isDark ? Color.white.opacity(0.14) : Color.white.opacity(0.22)
    }

    var glassFill: [Color] {
        if isDark {
            return [Color.white.opacity(0.10), Color.white.opacity(0.04)]
        }
        return [Color.white.opacity(0.18), Color.white.opacity(0.08)]
    }

    var glassStroke: [Color] {
        if isDark {
            return [
                Color.white.opacity(0.18),
                Color.white.opacity(0.08),
                Color.black.opacity(0.26),
            ]
        }
        return [
            Color.white.opacity(0.55),
            Color.white.opacity(0.22),
            Color.black.opacity(0.08),
        ]
    }

    var glassHighlight: [Color] {
        if isDark {
            return [Color.white.opacity(0.08), Color.white.opacity(0.00)]
        }
        return [Color.white.opacity(0.12), Color.white.opacity(0.00)]
    }

    var headerIconTileBackground: Color {
        isDark ? Color.white.opacity(0.12) : Color.white.opacity(0.32)
    }

    var headerIconTileStroke: Color {
        isDark ? Color.white.opacity(0.16) : Color.white.opacity(0.35)
    }

    var actionButtonBackground: Color {
        isDark ? Color.white.opacity(0.10) : Color.white.opacity(0.26)
    }

    var actionButtonStroke: Color {
        isDark ? Color.white.opacity(0.18) : Color.white.opacity(0.35)
    }

    var divider: Color {
        isDark ? Color.white.opacity(0.12) : Color.black.opacity(0.08)
    }

    var statChipBackground: Color {
        isDark ? Color.white.opacity(0.08) : Color.white.opacity(0.10)
    }

    var statChipStroke: Color {
        isDark ? Color.white.opacity(0.14) : Color.white.opacity(0.18)
    }

    var targetsContainerGradient: [Color] {
        if isDark {
            return [Color.white.opacity(0.10), Color.white.opacity(0.05)]
        }
        return [Color.white.opacity(0.16), Color.white.opacity(0.10)]
    }

    var targetsContainerStroke: Color {
        isDark ? Color.white.opacity(0.14) : Color.white.opacity(0.16)
    }

    var emptyStateTileBackground: Color {
        isDark ? Color.white.opacity(0.10) : Color.white.opacity(0.12)
    }

    var emptyStateTileStroke: Color {
        isDark ? Color.white.opacity(0.12) : Color.white.opacity(0.14)
    }

    var emptyStateSymbol: Color {
        isDark ? Color.secondary.opacity(0.82) : Color.secondary.opacity(0.70)
    }

    var runningRowBackground: Color {
        isDark ? Color.white.opacity(0.08) : Color.white.opacity(0.10)
    }

    var badgeBackground: Color {
        isDark ? Color.white.opacity(0.08) : Color.black.opacity(0.05)
    }

    var badgeStroke: Color {
        isDark ? Color.white.opacity(0.12) : Color.white.opacity(0.16)
    }

    var toastStroke: Color {
        isDark ? Color.white.opacity(0.18) : Color.white.opacity(0.28)
    }

    var toastShadow: Color {
        isDark ? Color.black.opacity(0.28) : Color.black.opacity(0.14)
    }
}
