import SwiftUI

// Image attachment rendering — the one path media flows through in both
// directions (REQ-IOS-015). Wire images are `{data: <base64>, media_type}`
// wherever they appear (user message content.images, tool result
// content.images, outbox entries); these views take that shape and follow
// the transcript-wide rule: decode failure degrades visibly, never to
// nothing.

/// One base64 image; renders a labeled placeholder when undecodable.
struct Base64ImageView: View {
    let base64: String
    @State private var decodedImage: UIImage?
    @State private var decodingFinished = false

    private struct DecodeResult: @unchecked Sendable {
        let image: UIImage?
    }

    var body: some View {
        Group {
            if let decodedImage {
                Image(uiImage: decodedImage)
                .resizable()
                .aspectRatio(contentMode: .fit)
            } else if decodingFinished {
                Label("image failed to decode", systemImage: "photo.badge.exclamationmark")
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .padding(8)
            } else {
                ProgressView()
                    .controlSize(.small)
                    .padding(8)
            }
        }
        .task(id: base64) {
            decodedImage = nil
            decodingFinished = false
            let result = await Task.detached(priority: .utility) {
                let image = Data(base64Encoded: base64).flatMap { UIImage(data: $0) }
                return DecodeResult(image: image)
            }.value
            guard !Task.isCancelled else { return }
            decodedImage = result.image
            decodingFinished = true
        }
    }
}

/// Horizontal strip of wire-shaped image payloads.
struct ImageStrip: View {
    let images: [JSONValue]
    var maxHeight: CGFloat = 160

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 6) {
                ForEach(Array(images.enumerated()), id: \.offset) { _, image in
                    Base64ImageView(base64: image["data"]?.stringValue ?? "")
                        .frame(maxHeight: maxHeight)
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                }
            }
        }
    }
}
