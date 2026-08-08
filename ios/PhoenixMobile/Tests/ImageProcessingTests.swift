import UIKit
import XCTest

@testable import PhoenixMobile

// Contract tests for the attachment capture path (REQ-IOS-015): picked
// photo data becomes a bounded, wire-ready ImagePayload, and undecodable
// input fails visibly (nil) rather than producing a corrupt payload.
final class ImageProcessingTests: XCTestCase {

    private func makeImageData(width: CGFloat, height: CGFloat) -> Data {
        let format = UIGraphicsImageRendererFormat.default()
        format.scale = 1
        let image = UIGraphicsImageRenderer(
            size: CGSize(width: width, height: height), format: format
        ).image { context in
            UIColor.systemRed.setFill()
            context.fill(CGRect(x: 0, y: 0, width: width, height: height))
        }
        return image.pngData()!
    }

    func testPayloadIsJpegAndBase64Decodable() {
        let payload = ImageProcessing.payload(fromPickedData: makeImageData(width: 100, height: 80))
        XCTAssertNotNil(payload)
        XCTAssertEqual(payload?.media_type, "image/jpeg")
        let decoded = payload.flatMap { Data(base64Encoded: $0.data) }
        XCTAssertNotNil(decoded)
        XCTAssertNotNil(decoded.flatMap { UIImage(data: $0) })
    }

    func testOversizedImageIsDownscaledToBoundedLongEdge() {
        let payload = ImageProcessing.payload(
            fromPickedData: makeImageData(width: 4000, height: 2000))
        let image = payload
            .flatMap { Data(base64Encoded: $0.data) }
            .flatMap { UIImage(data: $0) }
        XCTAssertNotNil(image)
        let longEdge = max(image!.size.width, image!.size.height)
        XCTAssertLessThanOrEqual(longEdge, ImageProcessing.maxDimension)
        // Aspect ratio preserved (2:1 within rounding).
        let ratio = image!.size.width / image!.size.height
        XCTAssertEqual(ratio, 2.0, accuracy: 0.05)
    }

    func testSmallImageIsNotUpscaled() {
        let original = UIImage(data: makeImageData(width: 100, height: 60))!
        let result = ImageProcessing.downscaled(original, maxDimension: 1568)
        XCTAssertEqual(result.size.width, 100, accuracy: 0.5)
        XCTAssertEqual(result.size.height, 60, accuracy: 0.5)
    }

    func testUndecodableDataProducesNilNotGarbage() {
        XCTAssertNil(ImageProcessing.payload(fromPickedData: Data("not an image".utf8)))
        XCTAssertNil(ImageProcessing.payload(fromPickedData: Data()))
    }

    func testEncodedSizeBudgetCountsWirePayloadBytes() {
        let payloads = [
            ImagePayload(data: String(repeating: "a", count: 12), media_type: "image/jpeg"),
            ImagePayload(data: String(repeating: "b", count: 30), media_type: "image/jpeg"),
        ]

        XCTAssertEqual(ImageProcessing.encodedSize(of: payloads), 42)
        XCTAssertGreaterThan(ImageProcessing.maxTotalEncodedBytes, 42)
    }
}
