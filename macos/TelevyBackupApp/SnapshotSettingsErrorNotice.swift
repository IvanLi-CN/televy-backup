import SwiftUI

struct SnapshotSettingsErrorNotice: View {
    let message: String

    var body: some View {
        Label(message, systemImage: "xmark.octagon")
            .font(.system(size: 12, weight: .medium))
            .foregroundStyle(.red)
    }
}
