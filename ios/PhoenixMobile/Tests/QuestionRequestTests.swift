import Foundation
import XCTest

@testable import PhoenixMobile

private final class QuestionRequestProtocol: URLProtocol {
    static var captured: [URLRequest] = []
    static var onGet: ((QuestionRequestProtocol) -> Void)?
    static var onRequest: ((QuestionRequestProtocol) -> Void)?

    override class func canInit(with request: URLRequest) -> Bool {
        request.url?.host == "auq-protocol.invalid"
    }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        Self.captured.append(request)
        if request.httpMethod == "GET" {
            if let onGet = Self.onGet { onGet(self) } else { snapshot() }
        } else if let onRequest = Self.onRequest { onRequest(self) } else { succeed() }
    }
    func snapshot(state: String = "{\"type\":\"idle\"}") {
        succeed(body: "{\"conversation\":{\"id\":\"conversation-a\",\"slug\":\"test\",\"state\":\(state)},\"messages\":[]}")
    }
    func succeed(status: Int = 200, body: String = "{\"success\":true}") {
        let response = HTTPURLResponse(url: request.url!, statusCode: status,
                                       httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
        client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
        client?.urlProtocol(self, didLoad: Data(body.utf8))
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
            XCTAssertNil(session.actionInFlight)
            XCTAssertTrue(session.acceptsChatMessage)
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
            XCTAssertNil(session.actionInFlight)
            XCTAssertTrue(session.acceptsChatMessage)
        }
    }

    @MainActor
    func testSuccessAndStaleMutationsStayResolvedThroughFailedRefreshAndCheckAgain() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [QuestionRequestProtocol.self]
        let api = PhoenixAPI(baseURL: URL(string: "https://auq-protocol.invalid")!,
                             password: nil, allowSelfSigned: false, configuration: configuration)!
        defer { QuestionRequestProtocol.onRequest = nil; QuestionRequestProtocol.onGet = nil }
        for status in [200, 409] {
            for action in [ConversationAction.respondToQuestions(requestId: "original", answers: [:]),
                           .dismissQuestion(requestId: "original")] {
                let session = ConversationSession(conversationId: "conversation-a", api: api,
                                                   connectivity: ConnectivityMonitor())
                try seed(session, requestId: "original")
                QuestionRequestProtocol.onRequest = { $0.succeed(status: status, body: status == 200 ? "{\"success\":true}" : "{\"error_type\":\"question_request_stale\"}") }
                QuestionRequestProtocol.onGet = { $0.succeed(status: 503, body: "unavailable") }
                try await XCTUnwrap(session.perform(action)).value
                XCTAssertTrue(session.questionResolvedWaitingForStream)
                XCTAssertNotNil(session.actionInFlight)
                XCTAssertFalse(session.canRetryQuestionOperation)
                XCTAssertTrue(session.canCheckQuestionStatus)
                XCTAssertNil(session.perform(action))
                QuestionRequestProtocol.onGet = nil
                try await XCTUnwrap(session.checkQuestionStatus()).value
                XCTAssertNil(session.actionInFlight)
                XCTAssertTrue(session.acceptsChatMessage)
            }
        }
    }

    @MainActor
    func testLateAuthoritativeRefreshCannotReplaceNewQuestion() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [QuestionRequestProtocol.self]
        let api = PhoenixAPI(baseURL: URL(string: "https://auq-protocol.invalid")!,
                             password: nil, allowSelfSigned: false, configuration: configuration)!
        let session = ConversationSession(conversationId: "conversation-a", api: api,
                                           connectivity: ConnectivityMonitor())
        defer { QuestionRequestProtocol.onGet = nil }
        try seed(session, requestId: "original")
        let received = expectation(description: "authoritative refresh received")
        var held: QuestionRequestProtocol?
        QuestionRequestProtocol.onGet = { request in held = request; received.fulfill() }
        let completion = try XCTUnwrap(session.perform(.dismissQuestion(requestId: "original")))
        await fulfillment(of: [received], timeout: 3)
        try seed(session, requestId: "newest")
        held?.snapshot()
        await completion.value
        XCTAssertEqual(session.typedState, .awaitingUserResponse(requestId: "newest", questions: []))
        XCTAssertNil(session.actionInFlight)
        XCTAssertFalse(session.acceptsChatMessage)
    }

    @MainActor
    func testStatusRefreshProjectsErrorAndWorkingMetadataToSessionAndList() async throws {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [QuestionRequestProtocol.self]
        let api = PhoenixAPI(baseURL: URL(string: "https://auq-protocol.invalid")!,
                             password: nil, allowSelfSigned: false, configuration: configuration)!
        defer { QuestionRequestProtocol.onGet = nil }
        for (state, mode, working) in [
            (#"{"type":"error","message":"Recovery failed","error_kind":"server_error"}"#, "error", false),
            (#"{"type":"awaiting_continuation"}"#, "working", true)
        ] {
            var updates: [Conversation] = []
            let session = ConversationSession(conversationId: "conversation-a", api: api,
                                               connectivity: ConnectivityMonitor(),
                                               onConversationUpdate: { updates.append($0) })
            try seed(session, requestId: "original")
            updates.removeAll()
            QuestionRequestProtocol.onGet = { request in
                request.succeed(body: """
                {"conversation":{"id":"conversation-a","slug":"refreshed","state":\(state),
                "state_updated_at":"2026-09-14T15:00:00Z","presentation_mode":"needs_action",
                "requires_action":true,"transcript_generation":99},
                "agent_working":\(working),"presentation_mode":"\(mode)","messages":["not a transcript projection"]}
                """)
            }
            try await XCTUnwrap(session.perform(.dismissQuestion(requestId: "original"))).value
            XCTAssertNil(session.actionInFlight)
            XCTAssertEqual(session.presentationMode, mode)
            XCTAssertEqual(session.agentWorking, working)
            XCTAssertEqual(session.conversation?.presentation_mode, mode)
            XCTAssertEqual(session.conversation?.requires_action, false)
            XCTAssertEqual(session.conversation?.state_updated_at, "2026-09-14T15:00:00Z")
            XCTAssertEqual(session.conversation?.slug, "refreshed")
            XCTAssertNotEqual(session.conversation?.transcript_generation, 99)
            XCTAssertTrue(session.messages.isEmpty)
            let listed = try XCTUnwrap(updates.last)
            XCTAssertEqual(listed.presentation_mode, mode)
            XCTAssertEqual(listed.requires_action, false)
            XCTAssertEqual(listed.state_updated_at, "2026-09-14T15:00:00Z")
            if mode == "error" {
                XCTAssertEqual(session.typedState, .error(message: "Recovery failed", kind: .serverError))
            } else {
                XCTAssertEqual(session.typedState, .awaitingContinuation)
            }
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
