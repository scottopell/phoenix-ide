import SwiftUI

struct RootView: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if model.isConfigured {
            ConversationListView()
        } else {
            SetupView()
        }
    }
}

/// Thin banner shown while the device has no network path. Sits above the
/// list/conversation content so the user always knows why data is stale.
struct OfflineBanner: View {
    @Environment(AppModel.self) private var model

    var body: some View {
        if !model.connectivity.isOnline {
            HStack(spacing: 6) {
                Image(systemName: "wifi.slash")
                Text("Offline — showing cached data, messages will queue")
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
            }
            .font(.caption)
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity)
            .padding(.vertical, 5)
            .background(.orange.gradient)
        }
    }
}
