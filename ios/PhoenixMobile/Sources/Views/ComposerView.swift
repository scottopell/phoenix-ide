import PhotosUI
import SwiftUI

/// Message input. Offline and supported mid-turn sends remain available;
/// state decisions and archive operations disable text and image submission.
struct ComposerView: View {
    @Environment(AppModel.self) private var model
    let session: ConversationSession
    @Binding var draft: String

    @State private var pickerItems: [PhotosPickerItem] = []
    @State private var attachments: [ImagePayload] = []
    @State private var attachmentError: String?
    @State private var isLoadingAttachments = false
    @State private var attachmentLoadGeneration = UUID()
    @State private var attachmentLoadTask: Task<Void, Never>?
    @FocusState private var focused: Bool

    private static let maxAttachments = 4

    var body: some View {
        VStack(spacing: 0) {
            Divider()

            if !attachments.isEmpty {
                attachmentChips
            }
            if let attachmentError {
                Text(attachmentError)
                    .font(.caption2)
                    .foregroundStyle(.red)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 12)
                    .padding(.top, 4)
            }

            HStack(alignment: .bottom, spacing: 8) {
                PhotosPicker(
                    selection: $pickerItems,
                    maxSelectionCount: Self.maxAttachments - attachments.count,
                    matching: .images
                ) {
                    Image(systemName: "photo.badge.plus")
                        .font(.title3)
                }
                .disabled(attachments.count >= Self.maxAttachments || isLoadingAttachments)

                TextField("Message", text: $draft, axis: .vertical)
                    .lineLimit(1...6)
                    .padding(.horizontal, 10)
                    .padding(.vertical, 8)
                    .background(Color(.secondarySystemBackground))
                    .clipShape(RoundedRectangle(cornerRadius: 16))
                    .focused($focused)
                    .accessibilityIdentifier("conversation.composer")

                if showsUnconfirmedStop {
                    Button {
                        session.perform(.cancel)
                    } label: {
                        Image(systemName: "stop.circle.fill")
                            .font(.title2)
                            .foregroundStyle(.red)
                    }
                    .disabled(!model.connectivity.isOnline)
                    .accessibilityIdentifier("conversation.cancel")
                }

                Button {
                    send()
                } label: {
                    // Orange while offline: the tap queues rather than sends.
                    Image(systemName: "arrow.up.circle.fill")
                        .font(.title2)
                        .foregroundStyle(model.connectivity.isOnline ? Color.accentColor : .orange)
                }
                .disabled(!canSend)
                .accessibilityIdentifier("conversation.send")
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
        }
        .background(.bar)
        .onChange(of: pickerItems) { _, items in
            guard !items.isEmpty else { return }
            attachmentLoadTask?.cancel()
            let generation = UUID()
            attachmentLoadGeneration = generation
            isLoadingAttachments = true
            attachmentLoadTask = Task {
                await loadPickedItems(items, generation: generation)
            }
        }
        .onDisappear {
            attachmentLoadTask?.cancel()
            attachmentLoadGeneration = UUID()
            isLoadingAttachments = false
        }
    }

    private var canSend: Bool {
        session.acceptsChatMessage
            && !isLoadingAttachments
            && (!draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
                || !attachments.isEmpty)
    }

    private var attachmentChips: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(Array(attachments.enumerated()), id: \.offset) { index, attachment in
                    ZStack(alignment: .topTrailing) {
                        Base64ImageView(base64: attachment.data)
                            .frame(height: 56)
                            .clipShape(RoundedRectangle(cornerRadius: 8))
                        Button {
                            attachments.remove(at: index)
                        } label: {
                            Image(systemName: "xmark.circle.fill")
                                .font(.caption)
                                .foregroundStyle(.white, .black.opacity(0.6))
                        }
                        .padding(2)
                    }
                }
            }
            .padding(.horizontal, 12)
            .padding(.top, 6)
        }
    }

    /// Load picked photos off the picker items, downscale/recompress, and
    /// stage them. Failures are surfaced, not swallowed — a photo that
    /// silently vanished from the send would be omission-as-data-loss in
    /// the other direction.
    private func loadPickedItems(_ items: [PhotosPickerItem], generation: UUID) async {
        attachmentError = nil
        var loaded: [ImagePayload] = []
        var failed = 0
        for item in items {
            guard !Task.isCancelled, generation == attachmentLoadGeneration else { return }
            guard attachments.count + loaded.count < Self.maxAttachments else { break }
            if let data = try? await item.loadTransferable(type: Data.self),
               let payload = await Task.detached(priority: .userInitiated, operation: {
                   ImageProcessing.payload(fromPickedData: data)
               }).value {
                loaded.append(payload)
            } else {
                failed += 1
            }
        }
        guard !Task.isCancelled, generation == attachmentLoadGeneration else { return }
        attachments.append(contentsOf: loaded)
        if failed > 0 {
            attachmentError = "Couldn't load \(failed) selected image\(failed == 1 ? "" : "s")."
        }
        pickerItems = []
        isLoadingAttachments = false
        attachmentLoadTask = nil
    }

    private func send() {
        guard canSend else { return }
        if session.send(text: draft, images: attachments) {
            draft = ""
            attachments = []
            attachmentError = nil
        }
    }

    private var showsUnconfirmedStop: Bool {
        guard session.typedState.isCancellable else { return false }
        if case .awaitingTaskApproval = session.typedState { return false }
        return true
    }
}
