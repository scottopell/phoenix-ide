import SwiftUI

/// Message input. Offline and supported mid-turn sends remain available;
/// state decisions and archive operations disable submission.
struct ComposerView: View {
    @Environment(AppModel.self) private var model
    let session: ConversationSession
    @Binding var draft: String

    @FocusState private var focused: Bool

    var body: some View {
        VStack(spacing: 0) {
            Divider()
            HStack(alignment: .bottom, spacing: 8) {
                TextField("Message", text: $draft, axis: .vertical)
                    .lineLimit(1...6)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(Color(.secondarySystemBackground))
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .focused($focused)

                if session.typedState.isCancellable {
                    Button {
                        session.perform(.cancel)
                    } label: {
                        Image(systemName: "stop.circle.fill")
                            .font(.title2)
                            .foregroundStyle(.red)
                    }
                    .disabled(!model.connectivity.isOnline)
                }

                Button {
                    send()
                } label: {
                    // Orange while offline: the tap queues rather than sends.
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.title2)
                        .foregroundStyle(model.connectivity.isOnline ? Color.accentColor : .orange)
                }
                .disabled(
                    draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                        || !session.acceptsChatMessage)
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
        }
        .background(.bar)
    }

    private func send() {
        guard session.acceptsChatMessage else { return }
        if session.send(text: draft) {
            draft = ""
        }
    }
}
