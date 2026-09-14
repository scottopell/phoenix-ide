import Foundation
import XCTest

@testable import PhoenixMobile

private final class QuestionRequestProtocol: URLProtocol {
    static var captured: [URLRequest] = []
    static var onRequest: ((QuestionRequestProtocol) -> Void)?

    override class func canInit(with request: URLRequest) -> Bool {
        request.url?.host == "auq-protocol.invalid"
    }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.captured.append(request)
        if let onRequest = Self.onRequest { onRequest(self) } else { succeed() }
    }
    func succeed() {
        let response = HTTPURLResponse(url: request.url!, statusCode: 200,
                                       httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data("{\"success\":true}".utf8))
        client?.urlProtocolDidFinishLoading(self)
    }
    override func stopLoading() {}
}

final class QuestionRequestTests: XCTestCase {
    @MainActor
    private func seed(_ session: ConversationSession, requestId: String) throws {
        let conversation = try JSONDecoder().decode(Conversation.self, from: Data("""
        {"id":"conversation-a","slug":"test","state":{"type":"awaiting_user_response",
         "request_id":"\(requestId)","tool_use_id":"reused-provider-id","questions":[]}}
        """.utf8))
        session.receive(.initSnapshot(.init(
            conversation: conversation, messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
    }

    @MainActor
    func testSuccessfulMutationsResolveWithoutStreamAndCannotResolveNewRequest() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [QuestionRequestProtocol.self]
        let api = PhoenixAPI(baseURL: URL(string: "https://auq-protocol.invalid")!,
                             password: nil, allowSelfSigned: false, configuration: configuration)!
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-question-success-\(UUID().uuidString)")
        defer { QuestionRequestProtocol.onRequest = nil }
        for action in [ConversationAction.respondToQuestions(requestId: "original", answers: [:]),
                       .dismissQuestion(requestId: "original")] {
            let session = ConversationSession(conversationId: "conversation-a", api: api,
                                               connectivity: ConnectivityMonitor())
            try seed(session, requestId: "original")
            QuestionRequestProtocol.onRequest = nil
            let completion = try XCTUnwrap(session.perform(action))
            await completion.value
            XCTAssertTrue(session.questionResolvedWaitingForStream)
            XCTAssertFalse(session.canRetryQuestionOperation)
            XCTAssertFalse(session.canCheckQuestionStatus)

            try seed(session, requestId: "next")
            XCTAssertFalse(session.questionResolvedWaitingForStream)
            let received = expectation(description: "new request received")
            var held: QuestionRequestProtocol?
            QuestionRequestProtocol.onRequest = { request in held = request; received.fulfill() }
            let next = try XCTUnwrap(session.perform(.dismissQuestion(requestId: "next")))
            await fulfillment(of: [received], timeout: 3)
            try seed(session, requestId: "newest")
            let newestReceived = expectation(description: "newest request received")
            var heldNewest: QuestionRequestProtocol?
            QuestionRequestProtocol.onRequest = { request in heldNewest = request; newestReceived.fulfill() }
            let newest = try XCTUnwrap(session.perform(.dismissQuestion(requestId: "newest")))
            await fulfillment(of: [newestReceived], timeout: 3)
            held?.succeed()
            await next.value
            XCTAssertFalse(session.questionResolvedWaitingForStream)
            XCTAssertEqual(session.actionInFlight?.questionRequestId, "newest")
            heldNewest?.succeed()
            await newest.value
            XCTAssertTrue(session.questionResolvedWaitingForStream)
            XCTAssertEqual(session.actionInFlight?.questionRequestId, "newest")
        }
    }

    func testBothMutationsCarryOriginatingRequestIdentity() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [QuestionRequestProtocol.self]
        QuestionRequestProtocol.captured = []
        let api = PhoenixAPI(baseURL: URL(string: "https://auq-protocol.invalid")!,
                             password: nil, allowSelfSigned: false, configuration: configuration)!
        try await api.respondToQuestion(conversationId: "conversation-a", requestId: "original",
                                        answers: ["Same question?": "first answer"])
        try await api.dismissQuestion(conversationId: "conversation-a", requestId: "original")
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
            XCTAssertEqual(payload["request_id"]?.stringValue, "original")
            if request.url?.lastPathComponent == "respond" {
                XCTAssertEqual(payload["answers"]?["Same question?"]?.stringValue, "first answer")
            }
        }
    }
}
