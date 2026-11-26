import SwiftUI

struct ErrorView: View {
    let origin: String?
    let message: String
    let suggestedOrigin: PhoenixOrigin?
    let onSwitchOrigin: ((PhoenixOrigin) -> Void)?
    let onRestart: () -> Void

    init(
        origin: String? = nil,
        message: String,
        suggestedOrigin: PhoenixOrigin? = nil,
        onSwitchOrigin: ((PhoenixOrigin) -> Void)? = nil,
        onRestart: @escaping () -> Void
    ) {
        self.origin = origin
        self.message = message
        self.suggestedOrigin = suggestedOrigin
        self.onSwitchOrigin = onSwitchOrigin
        self.onRestart = onRestart
    }

    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 40))
                .foregroundStyle(.orange)

            VStack(spacing: 4) {
                Text("Phoenix Connection")
                    .font(.title2)
                    .fontWeight(.semibold)
                if let origin {
                    Text(origin)
                        .font(.system(.caption, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .textSelection(.enabled)
                }
            }

            ScrollView {
                Text(message)
                    .font(.system(.caption, design: .monospaced))
                    .foregroundStyle(.secondary)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(12)
                    .background(.quaternary)
                    .clipShape(RoundedRectangle(cornerRadius: 6))
            }
            .frame(maxWidth: 560, maxHeight: 260)

            HStack(spacing: 10) {
                Button("Open Settings") {
                    NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
                }

                if let suggestedOrigin, let onSwitchOrigin {
                    Button("Retry") {
                        onRestart()
                    }
                    Button("Switch to \(suggestedOrigin.description)") {
                        onSwitchOrigin(suggestedOrigin)
                    }
                    .buttonStyle(.borderedProminent)
                } else {
                    Button("Retry") {
                        onRestart()
                    }
                    .buttonStyle(.borderedProminent)
                }
            }
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}
