import UIKit

/// Converts picked photos into wire-ready payloads. One deliberate path:
/// downscale to a bounded long edge and recompress as JPEG so a 48MP HEIC
/// photo doesn't become a 30MB base64 blob in the outbox file and the chat
/// POST (the LLM sees at most ~1.5k px usefully anyway).
enum ImageProcessing {
    static let maxDimension: CGFloat = 1568
    static let compressionQuality: CGFloat = 0.7
    static let maxTotalEncodedBytes = 20 * 1024 * 1024

    static func encodedSize(of payloads: [ImagePayload]) -> Int {
        payloads.reduce(0) { $0 + $1.data.utf8.count }
    }

    /// nil when the data isn't a decodable image.
    static func payload(fromPickedData data: Data) -> ImagePayload? {
        guard let image = UIImage(data: data) else { return nil }
        let scaled = downscaled(image, maxDimension: maxDimension)
        guard let jpeg = scaled.jpegData(compressionQuality: compressionQuality) else {
            return nil
        }
        return ImagePayload(
            data: jpeg.base64EncodedString(), media_type: "image/jpeg")
    }

    static func downscaled(_ image: UIImage, maxDimension: CGFloat) -> UIImage {
        let size = image.size
        let longEdge = max(size.width, size.height)
        guard longEdge > maxDimension, longEdge > 0 else { return image }
        let scale = maxDimension / longEdge
        let newSize = CGSize(width: size.width * scale, height: size.height * scale)
        // scale = 1: the output size is in pixels, not screen points — a 3x
        // device must not silently triple the pixel count back up.
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        return UIGraphicsImageRenderer(size: newSize, format: format).image { _ in
            image.draw(in: CGRect(origin: .zero, size: newSize))
        }
    }
}
