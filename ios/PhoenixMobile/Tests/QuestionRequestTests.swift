import Foundation
import XCTest

@testable import PhoenixMobile

private final class QuestionRequestProtocol: URLProtocol {
    static var captured: [URLRequest] = []
    static var status = 200
    static var body = #"{"success":true}"#

    override class func canInit(with request: URLRequest) -> Bool {
        request.url?.host == "auq-protocol.invalid"
    }

    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }

    override func startLoading() {
        Self.captured.append(request)
        let response = HTTPURLResponse(
            url: request.url!, statusCode: Self.status,
            httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(Self.body.utf8))
        client?.urlProtocolDidFinishLoading(self)
    }

    override func stopLoading() {}
}

final class QuestionRequestTests: XCTestCase {
    private func makeAPI() throws -> PhoenixAPI {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [QuestionRequestProtocol.self]
        return try XCTUnwrap(PhoenixAPI(
            baseURL: URL(string: "https://auq-protocol.invalid")!,
            password: nil,
            allowSelfSigned: false,
            configuration: configuration))
    }

    private func payload(_ request: URLRequest) throws -> [String: Any] {
        let data: Data
        if let body = request.httpBody {
            data = body
        } else {
            let stream = try XCTUnwrap(request.httpBodyStream)
            stream.open()
            defer { stream.close() }
            var body = Data()
            var buffer = [UInt8](repeating: 0, count: 4096)
            while stream.hasBytesAvailable {
                let count = stream.read(&buffer, maxLength: buffer.count)
                if count <= 0 { break }
                body.append(buffer, count: count)
            }
            data = body
        }
        return try XCTUnwrap(
            JSONSerialization.jsonObject(with: data) as? [String: Any])
    }

    override func setUp() {
        super.setUp()
        QuestionRequestProtocol.captured = []
        QuestionRequestProtocol.status = 200
        QuestionRequestProtocol.body = #"{"success":true}"#
    }

    func testIdentifiedAnswerAndDismissCarryRequestIdentity() async throws {
        let api = try makeAPI()

        try await api.respondToQuestion(
            conversationId: "conversation-a",
            requestId: "request-q2",
            answers: ["Which?": "Second"])
        try await api.dismissQuestion(
            conversationId: "conversation-a",
            requestId: "request-q2")

        let requests = QuestionRequestProtocol.captured
        XCTAssertEqual(requests.map { $0.url?.lastPathComponent }, ["respond", "dismiss-question"])
        XCTAssertEqual(try payload(requests[0])["request_id"] as? String, "request-q2")
        XCTAssertEqual(
            (try payload(requests[0])["answers"] as? [String: String])?["Which?"],
            "Second")
        XCTAssertEqual(try payload(requests[1])["request_id"] as? String, "request-q2")
    }

    func testLegacyAnswerAndDismissOmitRequestIdentity() async throws {
        let api = try makeAPI()

        try await api.respondToQuestion(
            conversationId: "conversation-a", requestId: nil, answers: [:])
        try await api.dismissQuestion(conversationId: "conversation-a", requestId: nil)

        XCTAssertEqual(QuestionRequestProtocol.captured.count, 2)
        for request in QuestionRequestProtocol.captured {
            XCTAssertNil(try payload(request)["request_id"])
        }
    }

    @MainActor
    func testConflictKeepsAuthoritativeQuestionStateVisible() async throws {
        QuestionRequestProtocol.status = 409
        QuestionRequestProtocol.body = #"{"error_type":"stale_question_request"}"#
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-question-conflict-\(UUID().uuidString)")
        let session = ConversationSession(
            conversationId: "conversation-a",
            api: try makeAPI(),
            connectivity: ConnectivityMonitor())
        let conversation = try JSONDecoder().decode(
            Conversation.self,
            from: Data(#"{"id":"conversation-a","slug":"test","state":{"type":"awaiting_user_response","request_id":"request-q2","questions":[]}}"#.utf8))
        session.receive(.initSnapshot(.init(
            conversation: conversation, messages: [], agentWorking: false,
            presentationMode: "needs_action", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        let completion = try XCTUnwrap(
            session.perform(.dismissQuestion(requestId: "request-q2")))
        await completion.value

        XCTAssertEqual(
            session.typedState,
            .awaitingUserResponse(questions: [], requestId: "request-q2"))
        XCTAssertNil(session.actionInFlight)
        XCTAssertNotNil(session.lastErrorToast)
        XCTAssertFalse(session.acceptsChatMessage)
    }
}
