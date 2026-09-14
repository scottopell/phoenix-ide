import Foundation
import XCTest

@testable import PhoenixMobile

private final class QuestionRequestProtocol: URLProtocol {
    static var captured: [URLRequest] = []

    override class func canInit(with request: URLRequest) -> Bool {
        request.url?.host == "auq-protocol.invalid"
    }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.captured.append(request)
        let response = HTTPURLResponse(url: request.url!, statusCode: 200,
                                       httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data("{\"success\":true}".utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

final class QuestionRequestTests: XCTestCase {
    func testBothMutationsCarryOriginatingRequestIdentity() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [QuestionRequestProtocol.self]
        QuestionRequestProtocol.captured = []
        let api = PhoenixAPI(baseURL: URL(string: "https://auq-protocol.invalid")!,
                             password: nil, allowSelfSigned: false, configuration: configuration)!
        try await api.respondToQuestion(conversationId: "conversation-a", toolUseId: "original",
                                        answers: ["Same question?": "first answer"])
        try await api.dismissQuestion(conversationId: "conversation-a", toolUseId: "original")
        let requests = QuestionRequestProtocol.captured
        XCTAssertEqual(requests.count, 2)
        XCTAssertEqual(requests.map { $0.url?.lastPathComponent }, ["respond", "dismiss-question"])
        for request in requests {
            let data: Data
            if let body = request.httpBody {
                data = body
            } else {
                let stream = try XCTUnwrap(request.httpBodyStream)
                stream.open()
                defer { stream.close() }
                var buffer = [UInt8](repeating: 0, count: 4096)
                var body = Data()
                while stream.hasBytesAvailable {
                    let count = stream.read(&buffer, maxLength: buffer.count)
                    if count <= 0 { break }
                    body.append(buffer, count: count)
                }
                data = body
            }
            let payload = try JSONDecoder().decode(JSONValue.self, from: data)
            XCTAssertEqual(payload["tool_use_id"]?.stringValue, "original")
            if request.url?.lastPathComponent == "respond" {
                XCTAssertEqual(payload["answers"]?["Same question?"]?.stringValue, "first answer")
            }
        }
    }
}
