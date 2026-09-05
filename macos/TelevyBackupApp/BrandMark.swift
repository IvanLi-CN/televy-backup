import AppKit
import SwiftUI

enum BrandMarkVariant: Hashable {
    case lightUI
    case darkUI
    case template

    var fileName: String {
        switch self {
        case .lightUI:
            return "televybackup-logo-ui-compact.svg"
        case .darkUI:
            return "televybackup-logo-dark-compact.svg"
        case .template:
            return "televybackup-logo-template.svg"
        }
    }
}

enum BrandMarkAsset {
    private static var cache: [BrandMarkVariant: NSImage] = [:]

    static func image(for variant: BrandMarkVariant) -> NSImage? {
        if let cached = cache[variant] {
            return cached
        }

        let image: NSImage?
        if let bundleURL = Bundle.main.url(
            forResource: variant.fileName,
            withExtension: nil,
            subdirectory: "Brand"
        ) {
            image = NSImage(contentsOf: bundleURL)
        } else {
            image = NSImage(contentsOf: sourceURL(for: variant))
        }

        guard let image else { return nil }
        image.isTemplate = variant == .template
        cache[variant] = image
        return image
    }

    private static func sourceURL(for variant: BrandMarkVariant) -> URL {
        URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("assets/brand")
            .appendingPathComponent(variant.fileName)
    }
}

struct TelevyBackupBrandMark: View {
    @Environment(\.colorScheme) private var colorScheme

    let size: CGFloat

    init(size: CGFloat) {
        self.size = size
    }

    var body: some View {
        Group {
            if let image = BrandMarkAsset.image(
                for: colorScheme == .dark ? .darkUI : .lightUI
            ) {
                Image(nsImage: image)
                    .resizable()
                    .renderingMode(.original)
                    .interpolation(.high)
                    .scaledToFit()
            } else {
                Color.clear
            }
        }
        .frame(width: size, height: size)
        .accessibilityLabel("TelevyBackup")
    }
}
