import Foundation

final class TestURLProtocol: URLProtocol {
    nonisolated(unsafe) private static var handlersByHost: [String: (owner: UUID, handler: (URLRequest) throws -> (HTTPURLResponse, Data))] = [:]
    private static let lock = NSLock()

    static func install(host: String, owner: UUID = UUID(), handler: @escaping (URLRequest) throws -> (HTTPURLResponse, Data)) -> UUID {
        lock.lock()
        handlersByHost[host] = (owner, handler)
        lock.unlock()
        return owner
    }

    static func uninstall(host: String, owner: UUID) {
        lock.lock()
        defer { lock.unlock() }
        guard let current = handlersByHost[host], current.owner == owner else { return }
        handlersByHost.removeValue(forKey: host)
    }

    static func removeAllHandlers() {
        lock.lock()
        handlersByHost.removeAll()
        lock.unlock()
    }

    private static func handler(for request: URLRequest) -> ((URLRequest) throws -> (HTTPURLResponse, Data))? {
        lock.lock()
        defer { lock.unlock() }
        return request.url.flatMap { handlersByHost[$0.host ?? ""]?.handler }
    }

    override class func canInit(with request: URLRequest) -> Bool {
        handler(for: request) != nil
    }
    override class func canonicalRequest(for request: URLRequest) -> URLRequest { request }
    override func startLoading() {
        guard let handler = Self.handler(for: request) else {
            client?.urlProtocol(self, didFailWithError: URLError(.badServerResponse))
            return
        }
        do {
            let (response, data) = try handler(request)
            client?.urlProtocol(self, didReceive: response, cacheStoragePolicy: .notAllowed)
            client?.urlProtocol(self, didLoad: data)
            client?.urlProtocolDidFinishLoading(self)
        } catch {
            client?.urlProtocol(self, didFailWithError: error)
        }
    }
    override func stopLoading() {}
}

import XCTest

@testable import PhoenixMobile

final class ConversationSessionReducerTests: XCTestCase {
    private struct RecordedRequest: Equatable, Sendable {
        let method: String
        let host: String?
        let path: String
        let body: String?
    }

    private final class RequestRecorder: @unchecked Sendable {
        private let lock = NSLock()
        private var requests: [RecordedRequest] = []

        func record(_ request: URLRequest) {
            let body = request.httpBody.flatMap { String(data: $0, encoding: .utf8) }
            let recorded = RecordedRequest(
                method: request.httpMethod ?? "",
                host: request.url?.host,
                path: request.url?.path ?? "",
                body: body)
            lock.lock()
            requests.append(recorded)
            lock.unlock()
        }

        func exactChatPosts(host: String) -> [RecordedRequest] {
            lock.lock()
            defer { lock.unlock() }
            return requests.filter { $0.method == "POST" && $0.host == host && $0.path == "/api/conversations/c1/chat" }
        }
    }

    private final class RequestCounter: @unchecked Sendable {
        private let lock = NSLock()
        private var count = 0

        func increment() {
            lock.lock()
            count += 1
            lock.unlock()
        }

        func snapshot() -> Int {
            lock.lock()
            defer { lock.unlock() }
            return count
        }
    }

    private actor TestLivenessTimeout {
        enum Failure: Error {
            case timedOut(String)
        }

        static func run<T: Sendable>(seconds: Double = 5, label: String, operation: @escaping @Sendable () async throws -> T) async throws -> T {
            try await withThrowingTaskGroup(of: T.self) { group in
                group.addTask {
                    try await operation()
                }
                group.addTask {
                    try await Task.sleep(for: .seconds(seconds))
                    throw Failure.timedOut(label)
                }
                let value = try await group.next()!
                group.cancelAll()
                return value
            }
        }
    }

    private actor ControlledTiming: SessionTiming {
        private var sleepCount = 0
        private var nextSleepOrdinal = 0
        private var enteredContinuations: [CheckedContinuation<Void, Never>] = []
        private var releaseContinuations: [Int: CheckedContinuation<Void, Error>] = [:]
        private var armedReleaseOrdinals: Set<Int> = []

        func waitForSleepEntry(count: Int = 1) async {
            if sleepCount >= count { return }
            await withCheckedContinuation { enteredContinuations.append($0) }
        }

        func releaseSleep(ordinal: Int = 1) async throws {
            if let continuation = releaseContinuations.removeValue(forKey: ordinal) {
                continuation.resume(returning: ())
                return
            }
            armedReleaseOrdinals.insert(ordinal)
        }

        func sleep(seconds: TimeInterval) async throws {
            _ = seconds
            sleepCount += 1
            nextSleepOrdinal += 1
            let ordinal = nextSleepOrdinal
            let continuations = enteredContinuations
            enteredContinuations.removeAll()
            continuations.forEach { $0.resume() }

            if armedReleaseOrdinals.remove(ordinal) != nil { return }
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { continuation in
                    releaseContinuations[ordinal] = continuation
                    if armedReleaseOrdinals.remove(ordinal) != nil,
                       let armedContinuation = releaseContinuations.removeValue(forKey: ordinal)
                    {
                        armedContinuation.resume(returning: ())
                    }
                }
            } onCancel: {
                Task {
                    await self.cancelSleep(ordinal: ordinal)
                }
            }
        }

        private func cancelSleep(ordinal: Int) {
            armedReleaseOrdinals.remove(ordinal)
            guard let continuation = releaseContinuations.removeValue(forKey: ordinal) else { return }
            continuation.resume(throwing: CancellationError())
        }
    }

    private struct ImmediateCancellationTiming: SessionTiming {
        func sleep(seconds: TimeInterval) async throws {
            _ = seconds
            try Task.checkCancellation()
            throw CancellationError()
        }
    }

    private final class ConversationUpdateGate: @unchecked Sendable {
        private let lock = NSLock()
        private var matched = false
        private var waiters: [CheckedContinuation<Void, Never>] = []

        func observe(_ conversation: Conversation) {
            guard conversation.product_conversation_id == "pc-new" else { return }
            let continuations = withLock { state -> [CheckedContinuation<Void, Never>] in
                if state.matched { return [] }
                state.matched = true
                let continuations = state.waiters
                state.waiters.removeAll()
                return continuations
            }
            continuations.forEach { $0.resume() }
        }

        func wait() async {
            if withLock({ $0.matched }) { return }
            await withCheckedContinuation { continuation in
                if withLock({ $0.matched }) {
                    continuation.resume()
                    return
                }
                withLockedState { state in
                    state.waiters.append(continuation)
                }
            }
        }

        func releaseAll() {
            let continuations = withLock { state -> [CheckedContinuation<Void, Never>] in
                let continuations = state.waiters
                state.waiters.removeAll()
                return continuations
            }
            continuations.forEach { $0.resume() }
        }

        private struct State {
            var matched = false
            var waiters: [CheckedContinuation<Void, Never>] = []
        }

        private func withLock<T>(_ body: (inout State) -> T) -> T {
            lock.lock()
            defer { lock.unlock() }
            var state = State(matched: matched, waiters: waiters)
            let result = body(&state)
            matched = state.matched
            waiters = state.waiters
            return result
        }

        private func withLockedState(_ body: (inout State) -> Void) {
            lock.lock()
            defer { lock.unlock() }
            var state = State(matched: matched, waiters: waiters)
            body(&state)
            matched = state.matched
            waiters = state.waiters
        }
    }

    private final class ScriptedStreamOpeningFactory: @unchecked Sendable {
        struct OpenRecord: Sendable {
            let ordinal: Int
            let apiIdentity: APIConfigurationIdentity
        }

        private enum Step {
            case blocked(AsyncThrowingStream<PhoenixEvent, Error>)
            case blockedIgnoringCancellation(AsyncThrowingStream<PhoenixEvent, Error>)
            case immediate(AsyncThrowingStream<PhoenixEvent, Error>)
        }

        private actor ScriptedOpener {
            let ordinal: Int
            let apiIdentity: APIConfigurationIdentity
            private let step: Step
            private let onOpened: @Sendable (Int, APIConfigurationIdentity) async -> Void
            private let onBlockedWait: @Sendable (Int) async throws -> Void
            private let onBlockedWaitIgnoringCancellation: @Sendable (Int) async -> Void

            init(
                ordinal: Int,
                apiIdentity: APIConfigurationIdentity,
                step: Step,
                onOpened: @escaping @Sendable (Int, APIConfigurationIdentity) async -> Void,
                onBlockedWait: @escaping @Sendable (Int) async throws -> Void,
                onBlockedWaitIgnoringCancellation: @escaping @Sendable (Int) async -> Void
            ) {
                self.ordinal = ordinal
                self.apiIdentity = apiIdentity
                self.step = step
                self.onOpened = onOpened
                self.onBlockedWait = onBlockedWait
                self.onBlockedWaitIgnoringCancellation = onBlockedWaitIgnoringCancellation
            }

            func openEventStream() async throws -> AsyncThrowingStream<PhoenixEvent, Error> {
                await onOpened(ordinal, apiIdentity)
                let stream: AsyncThrowingStream<PhoenixEvent, Error>
                switch step {
                case .immediate(let baseStream):
                    stream = baseStream
                case .blocked(let baseStream):
                    try await onBlockedWait(ordinal)
                    stream = baseStream
                case .blockedIgnoringCancellation(let baseStream):
                    await onBlockedWaitIgnoringCancellation(ordinal)
                    stream = baseStream
                }
                return stream
            }
        }

        private struct State {
            var steps: [Step]
            var makeCount = 0
            var openRecords: [OpenRecord] = []
            var openWaiters: [CheckedContinuation<Void, Never>] = []
            var blockedWaiters: [Int: CheckedContinuation<Void, Error>] = [:]
            var nonCooperativeBlockedWaiters: [Int: CheckedContinuation<Void, Never>] = [:]
            var armedBlockedReleases: Set<Int> = []
        }

        private let lock = NSLock()
        private var state: State

        init(
            steps: [AsyncThrowingStream<PhoenixEvent, Error>],
            blockedOrdinals: Set<Int>,
            blockedIgnoringCancellationOrdinals: Set<Int> = []
        ) {
            self.state = State(steps: steps.enumerated().map { index, stream in
                let ordinal = index + 1
                if blockedIgnoringCancellationOrdinals.contains(ordinal) {
                    return .blockedIgnoringCancellation(stream)
                }
                return blockedOrdinals.contains(ordinal) ? .blocked(stream) : .immediate(stream)
            })
        }

        private func makeOpener(for api: PhoenixAPI) -> ScriptedOpener {
            let (ordinal, step) = withLock { state in
                state.makeCount += 1
                let ordinal = state.makeCount
                let step = ordinal <= state.steps.count ? state.steps[ordinal - 1] : .immediate(AsyncThrowingStream { $0.finish() })
                return (ordinal, step)
            }
            return ScriptedOpener(
                ordinal: ordinal,
                apiIdentity: api.configurationIdentity,
                step: step,
                onOpened: { [weak self] ordinal, apiIdentity in
                    self?.recordOpen(ordinal: ordinal, apiIdentity: apiIdentity)
                },
                onBlockedWait: { [weak self] ordinal in
                    try await self?.waitForBlockedRelease(ordinal: ordinal)
                },
                onBlockedWaitIgnoringCancellation: { [weak self] ordinal in
                    await self?.waitForBlockedReleaseIgnoringCancellation(ordinal: ordinal)
                })
        }

        func waitForOpen(count: Int = 1) async {
            if withLock({ $0.openRecords.count >= count }) { return }
            await withCheckedContinuation { continuation in
                if withLock({ $0.openRecords.count >= count }) {
                    continuation.resume()
                    return
                }
                withLockedState { state in
                    state.openWaiters.append(continuation)
                }
            }
        }

        func releaseOpen(ordinal: Int) async throws {
            if let continuation = withLock({ $0.blockedWaiters.removeValue(forKey: ordinal) }) {
                continuation.resume(returning: ())
                return
            }
            if let continuation = withLock({ $0.nonCooperativeBlockedWaiters.removeValue(forKey: ordinal) }) {
                continuation.resume()
                return
            }
            withLockedState { state in
                state.armedBlockedReleases.insert(ordinal)
            }
        }

        func recordedAPIIdentities() -> [APIConfigurationIdentity] {
            withLock { state in
                state.openRecords.sorted { $0.ordinal < $1.ordinal }.map(\.apiIdentity)
            }
        }

        var openEventStream: ConversationEventStreamOpener {
            { api, _ in
                let opener = self.makeOpener(for: api)
                return try await opener.openEventStream()
            }
        }

        private func recordOpen(ordinal: Int, apiIdentity: APIConfigurationIdentity) {
            let waiters = withLock { state in
                state.openRecords.append(.init(ordinal: ordinal, apiIdentity: apiIdentity))
                let waiters = state.openWaiters
                state.openWaiters.removeAll()
                return waiters
            }
            waiters.forEach { $0.resume() }
        }

        private func waitForBlockedRelease(ordinal: Int) async throws {
            if withLock({ $0.armedBlockedReleases.remove(ordinal) != nil }) { return }
            try await withTaskCancellationHandler {
                try await withCheckedThrowingContinuation { continuation in
                    if withLock({ $0.armedBlockedReleases.remove(ordinal) != nil }) {
                        continuation.resume(returning: ())
                        return
                    }
                    withLockedState { state in
                        state.blockedWaiters[ordinal] = continuation
                    }
                }
            } onCancel: {
                self.cancelBlockedWait(ordinal: ordinal)
            }
        }

        private func waitForBlockedReleaseIgnoringCancellation(ordinal: Int) async {
            if withLock({ $0.armedBlockedReleases.remove(ordinal) != nil }) { return }
            await withCheckedContinuation { continuation in
                if withLock({ $0.armedBlockedReleases.remove(ordinal) != nil }) {
                    continuation.resume()
                    return
                }
                withLockedState { state in
                    state.nonCooperativeBlockedWaiters[ordinal] = continuation
                }
            }
        }

        private func cancelBlockedWait(ordinal: Int) {
            let continuation = withLock { state in
                state.armedBlockedReleases.remove(ordinal)
                return state.blockedWaiters.removeValue(forKey: ordinal)
            }
            continuation?.resume(throwing: CancellationError())
        }

        func releaseAllNonCooperativeWaiters() {
            let continuations = withLock { state in
                let continuations = Array(state.nonCooperativeBlockedWaiters.values)
                state.nonCooperativeBlockedWaiters.removeAll()
                return continuations
            }
            continuations.forEach { $0.resume() }
        }

        private func withLock<T>(_ body: (inout State) -> T) -> T {
            lock.lock()
            defer { lock.unlock() }
            return body(&state)
        }

        private func withLockedState(_ body: (inout State) -> Void) {
            lock.lock()
            defer { lock.unlock() }
            body(&state)
        }
    }

    @MainActor
    private func makeSession(
        onHardDeleted: @escaping @MainActor (ConversationSession.HardDeleteContext) async -> Void = { _ in },
        onConversationUpdate: ((Conversation) -> Void)? = nil,
        api: PhoenixAPI? = nil,
        retryTiming: any SessionTiming = LiveSessionTiming(),
        staleCheckTiming: any SessionTiming = LiveSessionTiming(),
        openEventStream: @escaping ConversationEventStreamOpener = defaultConversationEventStreamOpener,
        baseDirectory: URL? = nil,
        context: VersionedDiskContext? = nil,
        legacySnapshotPersistenceScope: PersistenceScopeIdentity? = nil,
        aggregateAuthority: String? = nil
    ) -> ConversationSession {
        let resolvedBaseDirectory = baseDirectory ?? FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let resolvedContext = context ?? DiskStore.versionedContext(baseDirectory: resolvedBaseDirectory)
        DiskStore.baseDirectory = resolvedBaseDirectory
        let snapshotDestination = resolvedBaseDirectory
            .appendingPathComponent("PhoenixMobile", isDirectory: true)
            .appendingPathComponent("conv-c1")
            .appendingPathExtension("json")
        return ConversationSession(
            conversationId: "c1",
            api: api ?? PhoenixAPI(
                baseURL: URL(string: "https://phoenix.invalid")!,
                password: nil,
                allowSelfSigned: false,
                configurationIdentity: APIConfigurationIdentity(serverURL: "https://phoenix.invalid", credentialGeneration: "test-default", trustSelfSigned: false))!,
            connectivity: ConnectivityMonitor(),
            outboxPersistence: OutboxPersistenceHandle.disk(conversationId: "c1", baseDirectory: resolvedBaseDirectory, context: resolvedContext),
            snapshotPersistence: resolvedContext.writer(destinationURL: snapshotDestination, version: ConversationSession.snapshotSchemaVersion),
            retryTiming: retryTiming,
            staleCheckTiming: staleCheckTiming,
            openEventStream: openEventStream,
            legacySnapshotPersistenceScope: legacySnapshotPersistenceScope,
            aggregateAuthority: aggregateAuthority,
            onConversationUpdate: onConversationUpdate,
            onHardDeleted: onHardDeleted)
    }

    private func json(_ raw: String) throws -> JSONValue {
        try JSONDecoder().decode(JSONValue.self, from: Data(raw.utf8))
    }

    private func conversation(
        id: String = "c1",
        aggregateId: String? = nil,
        state: String = "{\"type\":\"idle\"}"
    ) throws -> Conversation {
        let aggregateField = aggregateId.map { ",\"product_conversation_id\":\"\($0)\"" } ?? ""
        return try JSONDecoder().decode(
            Conversation.self,
            from: Data("{\"id\":\"\(id)\",\"slug\":\"\(id)\"\(aggregateField),\"state\":\(state)}".utf8))
    }

    private func message(id: String, type: String = "agent", content: String) throws -> Message {
        try JSONDecoder().decode(
            Message.self,
            from: Data("{\"message_id\":\"\(id)\",\"sequence_id\":2,\"message_type\":\"\(type)\",\"content\":\(content)}".utf8))
    }

    private func makeHTTPAPI(sendLog: @escaping (URLRequest) -> Void) -> (api: PhoenixAPI, registration: UUID) {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TestURLProtocol.self]
        let host = "phoenix.invalid"
        let registration = TestURLProtocol.install(host: host) { request in
            sendLog(request)
            let url = request.url!
            let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
            if url.path.contains("/chat") {
                return (response, Data(#"{"queued":false}"#.utf8))
            }
            return (response, Data("{}".utf8))
        }
        let session = URLSession(configuration: configuration)
        let api = PhoenixAPI(
            baseURL: URL(string: "https://\(host)")!,
            password: nil,
            allowSelfSigned: false,
            configurationIdentity: APIConfigurationIdentity(serverURL: "https://\(host)", credentialGeneration: host, trustSelfSigned: false),
            session: session,
            streamSession: session)!
        return (api, registration)
    }

    private func makeRecordedHTTPAPI(host: String, recorder: RequestRecorder) -> (api: PhoenixAPI, registration: UUID) {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TestURLProtocol.self]
        let registration = TestURLProtocol.install(host: host) { request in
            recorder.record(request)
            let url = request.url!
            let response = HTTPURLResponse(url: url, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
            if url.path == "/api/conversations/c1/chat" {
                return (response, Data(#"{"queued":false}"#.utf8))
            }
            return (response, Data("{}".utf8))
        }
        let session = URLSession(configuration: configuration)
        let api = PhoenixAPI(
            baseURL: URL(string: "https://\(host)")!,
            password: nil,
            allowSelfSigned: false,
            configurationIdentity: APIConfigurationIdentity(serverURL: "https://\(host)", credentialGeneration: host, trustSelfSigned: false),
            session: session,
            streamSession: session)!
        return (api, registration)
    }

    private final class SendGate: @unchecked Sendable {
        private let condition = NSCondition()
        private var enteredCount = 0
        private var enteredContinuations: [CheckedContinuation<Void, Never>] = []
        private var released = false

        func waitForEntry(count: Int = 1) async {
            await withCheckedContinuation { continuation in
                condition.lock()
                if enteredCount >= count {
                    condition.unlock()
                    continuation.resume()
                    return
                }
                enteredContinuations.append(continuation)
                condition.unlock()
            }
        }

        func enterAndWaitForRelease() {
            condition.lock()
            enteredCount += 1
            let continuations = enteredContinuations
            enteredContinuations.removeAll()
            condition.unlock()
            continuations.forEach { $0.resume() }

            condition.lock()
            while !released {
                condition.wait()
            }
            condition.unlock()
        }

        func release() {
            condition.lock()
            guard !released else {
                condition.unlock()
                return
            }
            released = true
            condition.broadcast()
            condition.unlock()
        }
    }

    @MainActor
    func testMessageUpdateWaitsForMessageIdentity() throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        session.receive(.messageUpdated(
            seq: 1, messageId: "m1", content: try json("[{\"type\":\"text\",\"text\":\"patched\"}]"),
            displayData: try json("{\"status\":\"running\",\"tool_starts\":{\"a\":1}}"),
            durationMs: nil,
            transcriptGeneration: 2))
        session.receive(.messageUpdated(
            seq: 2, messageId: "m1", content: nil,
            displayData: try json("{\"status\":\"completed\",\"tool_starts\":{\"b\":2}}"),
            durationMs: 321, transcriptGeneration: 2))
        session.receive(.message(
            seq: 3,
            message: try message(
                id: "m1", content: "[{\"type\":\"text\",\"text\":\"original\"}]")))

        XCTAssertEqual(
            session.messages[0].content.arrayValue?.first?["text"]?.stringValue,
            "patched")
        XCTAssertEqual(session.conversation?.transcript_generation, 2)
        XCTAssertEqual(session.messages[0].display_data?["status"]?.stringValue, "completed")
        XCTAssertEqual(session.messages[0].display_data?["tool_starts"]?["a"]?.numberValue, 1)
        XCTAssertEqual(session.messages[0].display_data?["tool_starts"]?["b"]?.numberValue, 2)
        XCTAssertEqual(session.messages[0].display_data?["duration_ms"]?.numberValue, 321)
    }

    @MainActor
    func testInitDropsPatchesFromThePreviousStreamBeforeReplay() throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        session.receive(.messageUpdated(
            seq: 1, messageId: "m1",
            content: try json("[{\"type\":\"text\",\"text\":\"stale patch\"}]"),
            displayData: nil, durationMs: nil, transcriptGeneration: nil))

        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 2,
            pendingAnchorSequenceId: 2, pendingEvents: [], pendingTruncated: false)))
        session.receive(.message(
            seq: 3,
            message: try message(
                id: "m1", content: "[{\"type\":\"text\",\"text\":\"fresh\"}]")))

        XCTAssertEqual(
            session.messages[0].content.arrayValue?.first?["text"]?.stringValue,
            "fresh")
    }

    @MainActor
    func testAgentDoneClearsUntypedWorkingMode() throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(state: "{\"type\":\"provisioning\"}"),
            messages: [], agentWorking: true, presentationMode: "working",
            lastSequenceId: 0, pendingAnchorSequenceId: 0,
            pendingEvents: [], pendingTruncated: false)))

        session.receive(.agentDone(seq: 1))

        XCTAssertEqual(session.typedState, .idle)
        XCTAssertEqual(session.presentationMode, "idle")
        XCTAssertFalse(session.agentWorking)
    }

    @MainActor
    func testHardDeleteClearsLocalTranscriptAndSignalsOwner() async throws {
        var deletedContext: ConversationSession.HardDeleteContext?
        let session = makeSession { deletedContext = $0 }
        session.receive(.initSnapshot(.init(
            conversation: try conversation(),
            messages: [try message(id: "m1", content: "[]")],
            agentWorking: false, presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let initialSnapshotSaved = await session.flushSnapshotPersistence()
        XCTAssertTrue(initialSnapshotSaved)
        XCTAssertTrue(( { if case .value = DiskStore.loadVersionedResult(ConversationSession.PersistedSnapshot.self, source: DiskStore.phoenixMobileDirectory(baseDirectory: DiskStore.baseDirectory).appendingPathComponent("conv-c1").appendingPathExtension("json"), version: ConversationSession.snapshotSchemaVersion) { return true } else { return false } }() ))

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "c1"))
        await session.clearCachedSnapshotAndWait()

        XCTAssertTrue(session.isHardDeleted)
        XCTAssertTrue(session.messages.isEmpty)
        XCTAssertNil(session.conversation)
        XCTAssertEqual(deletedContext?.conversationId, "c1")
        XCTAssertEqual(deletedContext?.aggregateAuthority, "c1")
        XCTAssertEqual(
            deletedContext?.configurationIdentity,
            APIConfigurationIdentity(
                serverURL: "https://phoenix.invalid",
                credentialGeneration: "test-default",
                trustSelfSigned: false))
    }

    @MainActor
    func testOfflineAvailabilityRequiresAnAuthoritativeCachedConversation() async throws {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let session = makeSession(baseDirectory: baseDirectory, context: context)
        let store = DiskConversationPersistenceStore(baseDirectory: baseDirectory, context: context)

        session.pauseForBackground()
        let emptySnapshotSaved = await session.flushSnapshotPersistence()
        XCTAssertTrue(emptySnapshotSaved)
        XCTAssertFalse(store.hasCachedSnapshot(conversationId: "c1"))

        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let authoritativeSnapshotSaved = await session.flushSnapshotPersistence()
        XCTAssertTrue(authoritativeSnapshotSaved)
        XCTAssertTrue(store.hasCachedSnapshot(conversationId: "c1"))
    }

    @MainActor
    func testAuthoritativeSnapshotReceiptRequiresSuccessfulAuthoritativePersistence() async throws {
        let session = makeSession(aggregateAuthority: "pc-1")

        XCTAssertNil(session.authoritativeSnapshotReceipt)
        XCTAssertFalse(session.canSendPersistedOutbox)

        session.pauseForBackground()
        let emptySnapshotSaved = await session.flushSnapshotPersistence()
        XCTAssertTrue(emptySnapshotSaved)
        XCTAssertNil(session.authoritativeSnapshotReceipt)
        XCTAssertFalse(session.canSendPersistedOutbox)

        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let authoritativeSnapshotSaved = await session.flushSnapshotPersistence()
        XCTAssertTrue(authoritativeSnapshotSaved)
        XCTAssertEqual(session.authoritativeSnapshotReceipt?.conversationId, "c1")
        XCTAssertEqual(session.authoritativeSnapshotReceipt?.aggregateId, "pc-1")
        XCTAssertEqual(
            session.authoritativeSnapshotReceipt?.configurationIdentity,
            PhoenixAPI(baseURL: URL(string: "https://phoenix.invalid")!, password: nil, allowSelfSigned: false, configurationIdentity: APIConfigurationIdentity(serverURL: "https://phoenix.invalid", credentialGeneration: "test-default", trustSelfSigned: false))!.configurationIdentity)
        XCTAssertTrue(session.canSendPersistedOutbox)
    }

    @MainActor
    func testHardDeleteClearsAuthoritativeSnapshotReceipt() async throws {
        let session = makeSession(aggregateAuthority: "pc-1")
        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        XCTAssertNotNil(session.authoritativeSnapshotReceipt)

        session.receive(.conversationHardDeleted(seq: 1, conversationId: "c1"))

        XCTAssertNil(session.authoritativeSnapshotReceipt)
        XCTAssertFalse(session.canSendPersistedOutbox)
    }

    @MainActor
    func testSameConfigurationScopedSnapshotEnablesColdRestartDrainExactlyOnce() async throws {
        let host = "same-config.invalid"
        let recorder = RequestRecorder()
        let (api, registration) = makeRecordedHTTPAPI(host: host, recorder: recorder)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let entry = OutboxEntry(
            localId: UUID().uuidString.lowercased(),
            conversationId: "c1",
            text: "send once",
            images: [],
            status: .pending,
            acceptedByServer: false,
            createdAt: Date(),
            acceptedAt: nil,
            lastError: nil,
            attemptCount: 0)
        let snapshotWriter = context.writer(
            destinationURL: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("conv-c1")
                .appendingPathExtension("json"),
            version: ConversationSession.snapshotSchemaVersion)
        let scopedSnapshot = ConversationSession.PersistedSnapshot(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [],
            lastSequenceId: 0,
            transcriptGeneration: nil,
            syncedAt: Date(),
            authoritative: ConversationSession.PersistedSnapshotAuthority(
                configurationIdentity: api.configurationIdentity,
                aggregateAuthority: "pc-1",
                syncedAt: Date()))
        _ = await snapshotWriter.save(scopedSnapshot, revision: snapshotWriter.reserveRevision())
        let outboxWriter = OutboxPersistenceHandle.disk(conversationId: "c1", baseDirectory: baseDirectory, context: context, aggregateAuthority: "pc-1")
        _ = await outboxWriter.save(PersistedOutboxEnvelope(scope: api.configurationIdentity.persistenceScope, aggregateAuthority: "pc-1", entries: [entry]), revision: outboxWriter.reserveRevision())

        let reopened = makeSession(api: api, baseDirectory: baseDirectory, context: context, aggregateAuthority: "pc-1")
        XCTAssertEqual(reopened.conversation?.id, "c1")
        XCTAssertTrue(reopened.canSendPersistedOutbox)
        let generation = try XCTUnwrap(reopened.drainOutbox())
        let result = await reopened.awaitDrainOutbox(generation: generation)

        XCTAssertTrue(result)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 1)
        XCTAssertEqual(reopened.outbox.entries.first?.localId, entry.localId)
        XCTAssertEqual(reopened.outbox.entries.first?.status, .pending)
        XCTAssertTrue(reopened.outbox.entries.first?.acceptedByServer ?? false)
        XCTAssertFalse(reopened.outbox.visibleEntries.isEmpty)
        let nextGeneration = try XCTUnwrap(reopened.drainOutbox())
        let nextResult = await reopened.awaitDrainOutbox(generation: nextGeneration)
        XCTAssertTrue(nextResult)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 1)
        _ = await reopened.outbox.flushPersistence()
        let reopenedAgain = makeSession(api: api, baseDirectory: baseDirectory, context: context, aggregateAuthority: "pc-1")
        XCTAssertTrue(reopenedAgain.outbox.entries.first?.acceptedByServer ?? false)
        let thirdGeneration = try XCTUnwrap(reopenedAgain.drainOutbox())
        let thirdResult = await reopenedAgain.awaitDrainOutbox(generation: thirdGeneration)
        XCTAssertTrue(thirdResult)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 1)
    }

    @MainActor
    func testDifferentConfigurationScopedSnapshotDoesNotEnableColdRestartSend() async throws {
        let host = "different-config.invalid"
        let recorder = RequestRecorder()
        let (originalAPI, registration) = makeRecordedHTTPAPI(host: host, recorder: recorder)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let writerContext = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let original = makeSession(api: originalAPI, baseDirectory: baseDirectory, context: writerContext, aggregateAuthority: "pc-1")
        _ = await original.outbox.enqueue(text: "held")
        original.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let originalPersisted = await original.flushSnapshotPersistence()
        XCTAssertTrue(originalPersisted)

        let outboxURL = DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
            .appendingPathComponent("outbox-c1")
            .appendingPathExtension("json")
        let persistedOutboxBeforeReopen = try Data(contentsOf: outboxURL)

        let differentAPI = PhoenixAPI(baseURL: URL(string: "https://other.invalid")!, password: nil, allowSelfSigned: false, configurationIdentity: APIConfigurationIdentity(serverURL: "https://other.invalid", credentialGeneration: "other.invalid", trustSelfSigned: false))!
        let reopened = makeSession(api: differentAPI, baseDirectory: baseDirectory, context: DiskStore.versionedContext(baseDirectory: baseDirectory), aggregateAuthority: "pc-1")

        XCTAssertNil(reopened.conversation)
        XCTAssertTrue(reopened.messages.isEmpty)
        XCTAssertFalse(reopened.canSendPersistedOutbox)
        XCTAssertNil(reopened.authoritativeSnapshotReceipt)
        let generation = try XCTUnwrap(reopened.drainOutbox())
        let completed = await reopened.awaitDrainOutbox(generation: generation)
        XCTAssertFalse(completed)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 0)
        XCTAssertTrue(reopened.outbox.entries.isEmpty)
        XCTAssertFalse(reopened.outbox.persistenceHealthy)
        XCTAssertEqual(try Data(contentsOf: outboxURL), persistedOutboxBeforeReopen)
    }

    @MainActor
    func testLegacyUnscopedSnapshotDoesNotEnableColdRestartSendUntilCurrentAuthoritativeInit() async throws {
        let host = "legacy-scope.invalid"
        let recorder = RequestRecorder()
        let (api, registration) = makeRecordedHTTPAPI(host: host, recorder: recorder)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let legacySnapshot = ConversationSession.PersistedSnapshot(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [try message(id: "m-legacy", content: "\"offline history\"")],
            lastSequenceId: 0,
            transcriptGeneration: nil,
            syncedAt: Date(),
            authoritative: nil)
        let snapshotWriter = context.writer(
            destinationURL: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("conv-c1")
                .appendingPathExtension("json"),
            version: ConversationSession.snapshotSchemaVersion)
        _ = await snapshotWriter.save(legacySnapshot, revision: snapshotWriter.reserveRevision())
        let pending = OutboxEntry(
            localId: UUID().uuidString.lowercased(),
            conversationId: "c1",
            text: "held",
            images: [],
            status: .pending,
            acceptedByServer: false,
            createdAt: Date(),
            acceptedAt: nil,
            lastError: nil,
            attemptCount: 0)
        let previousBaseDirectory = DiskStore.baseDirectory
        DiskStore.baseDirectory = baseDirectory
        defer { DiskStore.baseDirectory = previousBaseDirectory }
        XCTAssertTrue(DiskStore.saveVersioned([pending], name: "outbox-c1", version: 1))

        let reopened = makeSession(
            api: api,
            baseDirectory: baseDirectory,
            context: context,
            legacySnapshotPersistenceScope: api.configurationIdentity.persistenceScope,
            aggregateAuthority: "pc-1")
        XCTAssertEqual(reopened.conversation?.id, "c1")
        XCTAssertEqual(reopened.messages.map(\.message_id), ["m-legacy"])
        XCTAssertNil(reopened.authoritativeSnapshotReceipt)
        XCTAssertTrue(reopened.outbox.entries.isEmpty)
        XCTAssertFalse(reopened.outbox.persistenceHealthy)
        XCTAssertFalse(reopened.canSendPersistedOutbox)
        let blockedGeneration = try XCTUnwrap(reopened.drainOutbox())
        let blockedCompleted = await reopened.awaitDrainOutbox(generation: blockedGeneration)
        XCTAssertFalse(blockedCompleted)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 0)
        if case .unreadable = DiskStore.loadVersionedResult(
            PersistedOutboxEnvelope.self,
            source: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("outbox-c1")
                .appendingPathExtension("json"),
            version: Outbox.schemaVersion)
        {} else {
            XCTFail("expected preserved unreadable schema-v1 outbox")
        }

        reopened.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let reopenedPersisted = await reopened.flushSnapshotPersistence()
        XCTAssertTrue(reopenedPersisted)
        let generation = try XCTUnwrap(reopened.drainOutbox())
        let drainCompleted = await reopened.awaitDrainOutbox(generation: generation)
        XCTAssertTrue(drainCompleted)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 0)
        if case .unreadable = DiskStore.loadVersionedResult(
            PersistedOutboxEnvelope.self,
            source: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("outbox-c1")
                .appendingPathExtension("json"),
            version: Outbox.schemaVersion)
        {} else {
            XCTFail("expected unreadable schema-v1 outbox to remain unmodified")
        }
    }

    @MainActor
    func testLegacySnapshotWithoutProvenInstallationScopeFailsClosed() async throws {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let snapshotWriter = context.writer(
            destinationURL: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("conv-c1")
                .appendingPathExtension("json"),
            version: ConversationSession.snapshotSchemaVersion)
        _ = await snapshotWriter.save(
            ConversationSession.PersistedSnapshot(
                conversation: try conversation(id: "c1", aggregateId: "pc-1"),
                messages: [try message(id: "m1", content: "\"legacy\"")],
                lastSequenceId: 1,
                transcriptGeneration: nil,
                syncedAt: Date(),
                authoritative: nil),
            revision: snapshotWriter.reserveRevision())

        let reopened = makeSession(
            baseDirectory: baseDirectory,
            context: context,
            legacySnapshotPersistenceScope: nil,
            aggregateAuthority: "pc-1")

        XCTAssertNil(reopened.conversation)
        XCTAssertTrue(reopened.messages.isEmpty)
        XCTAssertNil(reopened.authoritativeSnapshotReceipt)
        XCTAssertFalse(reopened.canSendPersistedOutbox)
    }

    @MainActor
    func testReplaceAPIInvalidatesAuthoritativeSnapshotReceipt() async throws {
        let session = makeSession(aggregateAuthority: "pc-1")
        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        XCTAssertNotNil(session.authoritativeSnapshotReceipt)

        let replacement = PhoenixAPI(
            baseURL: URL(string: "https://phoenix.example")!,
            password: nil,
            allowSelfSigned: false,
            configurationIdentity: APIConfigurationIdentity(serverURL: "https://phoenix.example", credentialGeneration: "phoenix.example", trustSelfSigned: false))!
        session.replaceAPI(replacement)

        XCTAssertNil(session.authoritativeSnapshotReceipt)
        XCTAssertFalse(session.canSendPersistedOutbox)
    }

    @MainActor
    func testOrdinaryConversationSnapshotUnlocksPersistedOutbox() async throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: nil),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        let persisted = await session.flushSnapshotPersistence()

        XCTAssertTrue(persisted)
        XCTAssertEqual(session.authoritativeSnapshotReceipt?.conversationId, "c1")
        XCTAssertEqual(session.authoritativeSnapshotReceipt?.aggregateId, "c1")
        XCTAssertTrue(session.canSendPersistedOutbox)
    }

    @MainActor
    func testOrdinaryConversationColdRehydrateDrainsExactlyOnce() async throws {
        let host = "ordinary-rehydrate.invalid"
        let recorder = RequestRecorder()
        let (api, registration) = makeRecordedHTTPAPI(host: host, recorder: recorder)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let entry = OutboxEntry(
            localId: UUID().uuidString.lowercased(),
            conversationId: "c1",
            text: "ordinary send once",
            images: [],
            status: .pending,
            acceptedByServer: false,
            createdAt: Date(),
            acceptedAt: nil,
            lastError: nil,
            attemptCount: 0)
        let snapshotWriter = context.writer(
            destinationURL: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("conv-c1")
                .appendingPathExtension("json"),
            version: ConversationSession.snapshotSchemaVersion)
        let ordinarySnapshot = ConversationSession.PersistedSnapshot(
            conversation: try conversation(id: "c1", aggregateId: nil),
            messages: [],
            lastSequenceId: 0,
            transcriptGeneration: nil,
            syncedAt: Date(),
            authoritative: ConversationSession.PersistedSnapshotAuthority(
                configurationIdentity: api.configurationIdentity,
                aggregateAuthority: "c1",
                syncedAt: Date()))
        _ = await snapshotWriter.save(ordinarySnapshot, revision: snapshotWriter.reserveRevision())
        let outboxWriter = OutboxPersistenceHandle.disk(conversationId: "c1", baseDirectory: baseDirectory, context: context, aggregateAuthority: "c1")
        _ = await outboxWriter.save(PersistedOutboxEnvelope(scope: api.configurationIdentity.persistenceScope, aggregateAuthority: "c1", entries: [entry]), revision: outboxWriter.reserveRevision())

        let reopened = makeSession(api: api, baseDirectory: baseDirectory, context: context, aggregateAuthority: "c1")
        XCTAssertTrue(reopened.canSendPersistedOutbox)
        let generation = try XCTUnwrap(reopened.drainOutbox())
        let drained = await reopened.awaitDrainOutbox(generation: generation)
        XCTAssertTrue(drained)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 1)
        XCTAssertEqual(reopened.outbox.entries.first?.localId, entry.localId)
        XCTAssertTrue(reopened.outbox.entries.first?.acceptedByServer ?? false)
        XCTAssertFalse(reopened.outbox.visibleEntries.isEmpty)

        let nextGeneration = try XCTUnwrap(reopened.drainOutbox())
        let nextDrained = await reopened.awaitDrainOutbox(generation: nextGeneration)
        XCTAssertTrue(nextDrained)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 1)
        _ = await reopened.outbox.flushPersistence()
        let reopenedAgain = makeSession(api: api, baseDirectory: baseDirectory, context: context, aggregateAuthority: "c1")
        XCTAssertTrue(reopenedAgain.outbox.entries.first?.acceptedByServer ?? false)
        let thirdGeneration = try XCTUnwrap(reopenedAgain.drainOutbox())
        let thirdDrained = await reopenedAgain.awaitDrainOutbox(generation: thirdGeneration)
        XCTAssertTrue(thirdDrained)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 1)
    }

    @MainActor
    func testForeignConversationIdAuthorityDoesNotUnlockPersistedOutbox() async throws {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let session = makeSession(baseDirectory: baseDirectory, context: context)
        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "other-row", aggregateId: nil),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))

        let persisted = await session.flushSnapshotPersistence()

        XCTAssertTrue(persisted)
        XCTAssertNil(session.authoritativeSnapshotReceipt)
        XCTAssertFalse(session.canSendPersistedOutbox)
    }

    @MainActor
    func testForgedOrdinarySnapshotCannotSelfAssertForeignAggregateAuthorityOnRehydrate() async throws {
        let host = "ordinary-forged.invalid"
        let recorder = RequestRecorder()
        let (api, registration) = makeRecordedHTTPAPI(host: host, recorder: recorder)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let snapshotWriter = context.writer(
            destinationURL: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("conv-c1")
                .appendingPathExtension("json"),
            version: ConversationSession.snapshotSchemaVersion)
        let forged = ConversationSession.PersistedSnapshot(
            conversation: try conversation(id: "c1", aggregateId: "pc-forged"),
            messages: [],
            lastSequenceId: 0,
            transcriptGeneration: nil,
            syncedAt: Date(),
            authoritative: .init(
                configurationIdentity: api.configurationIdentity,
                aggregateAuthority: "c1",
                syncedAt: Date()))
        _ = await snapshotWriter.save(forged, revision: snapshotWriter.reserveRevision())
        let outboxWriter = OutboxPersistenceHandle.disk(conversationId: "c1", baseDirectory: baseDirectory, context: context, aggregateAuthority: "c1")
        _ = await outboxWriter.save(PersistedOutboxEnvelope(scope: nil, aggregateAuthority: "c1", entries: [
            OutboxEntry(localId: UUID().uuidString.lowercased(), conversationId: "c1", text: "held", images: [], status: .pending, acceptedByServer: false, createdAt: Date(), acceptedAt: nil, lastError: nil, attemptCount: 0)
        ]), revision: outboxWriter.reserveRevision())

        let reopened = makeSession(api: api, baseDirectory: baseDirectory, context: context, aggregateAuthority: "c1")

        XCTAssertNil(reopened.authoritativeSnapshotReceipt)
        XCTAssertFalse(reopened.canSendPersistedOutbox)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 0)
    }

    @MainActor
    func testForeignAggregateIdentityDoesNotUnlockPersistedOutboxOnRehydrate() async throws {
        let host = "foreign-aggregate.invalid"
        let recorder = RequestRecorder()
        let (api, registration) = makeRecordedHTTPAPI(host: host, recorder: recorder)
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let snapshotWriter = context.writer(
            destinationURL: DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
                .appendingPathComponent("conv-c1")
                .appendingPathExtension("json"),
            version: ConversationSession.snapshotSchemaVersion)
        let forged = ConversationSession.PersistedSnapshot(
            conversation: try conversation(id: "c1", aggregateId: "pc-forged"),
            messages: [],
            lastSequenceId: 0,
            transcriptGeneration: nil,
            syncedAt: Date(),
            authoritative: .init(configurationIdentity: api.configurationIdentity, aggregateAuthority: "pc-forged", syncedAt: Date()))
        _ = await snapshotWriter.save(forged, revision: snapshotWriter.reserveRevision())
        let outboxWriter = OutboxPersistenceHandle.disk(conversationId: "c1", baseDirectory: baseDirectory, context: context, aggregateAuthority: "pc-1")
        _ = await outboxWriter.save(PersistedOutboxEnvelope(scope: nil, aggregateAuthority: "pc-1", entries: [
            OutboxEntry(localId: UUID().uuidString.lowercased(), conversationId: "c1", text: "held", images: [], status: .pending, acceptedByServer: false, createdAt: Date(), acceptedAt: nil, lastError: nil, attemptCount: 0)
        ]), revision: outboxWriter.reserveRevision())

        let reopened = makeSession(api: api, baseDirectory: baseDirectory, context: context, aggregateAuthority: "pc-1")

        XCTAssertNil(reopened.authoritativeSnapshotReceipt)
        XCTAssertFalse(reopened.canSendPersistedOutbox)
        XCTAssertEqual(recorder.exactChatPosts(host: host).count, 0)
    }

    @MainActor
    func testAuthoritativeAsyncSaveFollowedByOrdinaryFlushPreservesAuthorityScope() async throws {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-session-tests-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let (api, registration) = makeHTTPAPI(sendLog: { _ in })
        defer { TestURLProtocol.uninstall(host: "phoenix.invalid", owner: registration) }
        let session = makeSession(api: api, baseDirectory: baseDirectory, context: context, aggregateAuthority: "pc-1")
        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        session.pauseForBackground()
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)

        let reopened = makeSession(api: api, baseDirectory: baseDirectory, context: context, aggregateAuthority: "pc-1")
        XCTAssertTrue(reopened.canSendPersistedOutbox)
        XCTAssertNotNil(reopened.authoritativeSnapshotReceipt)
    }

    @MainActor
    func testSnapshotRemovalFencesPendingSaves() async throws {
        let session = makeSession()
        session.receive(.initSnapshot(.init(
            conversation: try conversation(), messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        session.stop()

        await session.clearCachedSnapshotAndWait()

        XCTAssertFalse(( { if case .value = DiskStore.loadVersionedResult(ConversationSession.PersistedSnapshot.self, source: DiskStore.phoenixMobileDirectory(baseDirectory: DiskStore.baseDirectory).appendingPathComponent("conv-c1").appendingPathExtension("json"), version: ConversationSession.snapshotSchemaVersion) { return true } else { return false } }() ))
    }

    @MainActor
    func testReconnectInitEmitsAggregateTopologyInvalidationFromPreviousAggregateAndState() throws {
        let session = makeSession()
        var invalidations: [ProductConversationTopologyInvalidation] = []
        session.setSessionEventObserver { event in
            if case .aggregateTopologyInvalidated(let invalidation) = event {
                invalidations.append(invalidation)
            }
        }

        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "row-1", aggregateId: "agg-old", state: "{\"type\":\"working\"}"),
            messages: [], agentWorking: true, presentationMode: "working",
            lastSequenceId: 0, pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false,
            transcriptGeneration: 1)))
        invalidations.removeAll()

        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "row-1", aggregateId: "agg-new", state: "{\"type\":\"idle\"}"),
            messages: [], agentWorking: false, presentationMode: "idle",
            lastSequenceId: 0, pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false,
            transcriptGeneration: 2)))

        XCTAssertEqual(invalidations.count, 1)
        XCTAssertEqual(invalidations.first?.aggregateIdentity, "agg-new")
        if case .aggregateIdentityChanged(let previous, let current) = invalidations.first?.reason {
            XCTAssertEqual(previous, "agg-old")
            XCTAssertEqual(current, "agg-new")
        } else {
            XCTFail("expected aggregate identity change")
        }
    }

    @MainActor
    func testCanonicalAuthoritativeMessageReconcilesOptimisticEntry() async throws {
        let session = makeSession()
        let entry = await session.outbox.enqueue(text: "sent once")!

        session.receive(.initSnapshot(.init(
            conversation: try conversation(),
            messages: [try message(
                id: "c1:\(entry.localId)", type: "user",
                content: "{\"text\":\"sent once\"}")],
            agentWorking: true, presentationMode: "working", lastSequenceId: 2,
            pendingAnchorSequenceId: 2, pendingEvents: [], pendingTruncated: false)))
        session.receive(.stateChange(
            seq: 3, state: .string("idle"), presentationMode: "idle",
            stateUpdatedAt: nil))
        let latestSnapshotSaved = await session.flushSnapshotPersistence()

        XCTAssertTrue(latestSnapshotSaved)
        XCTAssertTrue(session.outbox.visibleEntries.isEmpty)
        XCTAssertEqual(session.outbox.entries.first?.status, .reconciled)
    }

    @MainActor
    func testBlockedDrainSuccessAfterConfigurationInvalidationDoesNotMutateOutbox() async throws {
        let gate = SendGate()
        let postCount = RequestCounter()
        let host = "blocked-config.invalid"
        let registration = TestURLProtocol.install(host: host) { request in
            if request.url?.path.contains("/chat") == true {
                postCount.increment()
                gate.enterAndWaitForRelease()
                let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
                return (response, Data(#"{"queued":false}"#.utf8))
            }
            let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
            return (response, Data("{}".utf8))
        }
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TestURLProtocol.self]
        let urlSession = URLSession(configuration: configuration)
        let api = PhoenixAPI(
            baseURL: URL(string: "https://\(host)")!,
            password: nil,
            allowSelfSigned: false,
            configurationIdentity: APIConfigurationIdentity(serverURL: "https://\(host)", credentialGeneration: host, trustSelfSigned: false),
            session: urlSession,
            streamSession: urlSession)!
        let session = makeSession(api: api, aggregateAuthority: "pc-1")
        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        _ = await session.outbox.enqueue(text: "send once")

        guard let generation = session.drainOutbox() else {
            XCTFail("expected first drain generation")
            return
        }
        try await TestLivenessTimeout.run(label: "waitForEntry") {
            await gate.waitForEntry()
        }
        session.invalidateConfiguration()
        gate.release()
        let drained = try await TestLivenessTimeout.run(label: "awaitDrainOutbox") {
            await session.awaitDrainOutbox(generation: generation)
        }

        XCTAssertFalse(drained)
        XCTAssertEqual(postCount.snapshot(), 1)
        XCTAssertFalse(session.outbox.entries[0].acceptedByServer)
        XCTAssertEqual(session.outbox.entries[0].attemptCount, 1)
    }

    @MainActor
    func testBlockedDrainSuccessAfterHardDeleteDoesNotMutateOutbox() async throws {
        let gate = SendGate()
        let postCount = RequestCounter()
        let host = "blocked-delete.invalid"
        let registration = TestURLProtocol.install(host: host) { request in
            if request.url?.path.contains("/chat") == true {
                postCount.increment()
                gate.enterAndWaitForRelease()
                let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
                return (response, Data(#"{"queued":false}"#.utf8))
            }
            let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
            return (response, Data("{}".utf8))
        }
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TestURLProtocol.self]
        let urlSession = URLSession(configuration: configuration)
        let api = PhoenixAPI(
            baseURL: URL(string: "https://\(host)")!,
            password: nil,
            allowSelfSigned: false,
            configurationIdentity: APIConfigurationIdentity(serverURL: "https://\(host)", credentialGeneration: host, trustSelfSigned: false),
            session: urlSession,
            streamSession: urlSession)!
        let session = makeSession(api: api, aggregateAuthority: "pc-1")
        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        _ = await session.outbox.enqueue(text: "send once")

        guard session.drainOutbox() != nil else {
            XCTFail("expected first drain generation")
            return
        }
        await gate.waitForEntry()
        guard let task = session.currentDrainTaskForTesting() else {
            XCTFail("expected current drain task")
            return
        }
        session.receive(.conversationHardDeleted(seq: 1, conversationId: "c1"))
        gate.release()
        _ = await task.value

        XCTAssertEqual(postCount.snapshot(), 1)
        XCTAssertEqual(session.outbox.entries.count, 1)
        XCTAssertTrue(session.isHardDeleted)
    }

    @MainActor
    func testStoppedBlockedDrainContinuesAcrossRestartWithoutSecondPost() async throws {
        let gate = SendGate()
        let postCount = RequestCounter()
        let host = "blocked-restart.invalid"
        let registration = TestURLProtocol.install(host: host) { request in
            if request.url?.path.contains("/chat") == true {
                postCount.increment()
                gate.enterAndWaitForRelease()
                let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
                return (response, Data(#"{"queued":false}"#.utf8))
            }
            let response = HTTPURLResponse(url: request.url!, statusCode: 200, httpVersion: nil, headerFields: ["Content-Type": "application/json"])!
            return (response, Data("{}".utf8))
        }
        defer { TestURLProtocol.uninstall(host: host, owner: registration) }
        let configuration = URLSessionConfiguration.ephemeral
        configuration.protocolClasses = [TestURLProtocol.self]
        let urlSession = URLSession(configuration: configuration)
        let api = PhoenixAPI(
            baseURL: URL(string: "https://\(host)")!,
            password: nil,
            allowSelfSigned: false,
            configurationIdentity: APIConfigurationIdentity(serverURL: "https://\(host)", credentialGeneration: host, trustSelfSigned: false),
            session: urlSession,
            streamSession: urlSession)!
        let session = makeSession(api: api, aggregateAuthority: "pc-1")
        session.receive(.initSnapshot(.init(
            conversation: try conversation(id: "c1", aggregateId: "pc-1"),
            messages: [], agentWorking: false,
            presentationMode: "idle", lastSequenceId: 0,
            pendingAnchorSequenceId: 0, pendingEvents: [], pendingTruncated: false)))
        let persisted = await session.flushSnapshotPersistence()
        XCTAssertTrue(persisted)
        _ = await session.outbox.enqueue(text: "send once")

        guard let g1 = session.drainOutbox() else {
            XCTFail("expected first drain generation")
            return
        }
        await gate.waitForEntry()
        session.stop()
        guard let restartedGeneration = session.drainOutbox() else {
            XCTFail("expected restarted drain generation")
            return
        }
        XCTAssertEqual(restartedGeneration, g1)
        gate.release()
        let drainedG1 = await session.awaitDrainOutbox(generation: g1)
        XCTAssertTrue(drainedG1)

        XCTAssertEqual(postCount.snapshot(), 1)
        XCTAssertEqual(session.outbox.entries.count, 1)
        XCTAssertTrue(session.outbox.entries[0].acceptedByServer)
        XCTAssertEqual(session.outbox.entries[0].attemptCount, 1)
    }

    @MainActor
    func testStoppedStreamOpenReturningLateDoesNotSetLiveOrApplyEvent() async throws {
        let stream = AsyncThrowingStream<PhoenixEvent, Error> { continuation in
            continuation.yield(.initSnapshot(.init(
                conversation: try! self.conversation(id: "c1", aggregateId: "pc-1"),
                messages: [], agentWorking: false,
                presentationMode: "idle", lastSequenceId: 1,
                pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
            continuation.finish()
        }
        let openerFactory = ScriptedStreamOpeningFactory(steps: [stream], blockedOrdinals: [1])
        let session = makeSession(
            retryTiming: ImmediateCancellationTiming(),
            staleCheckTiming: ImmediateCancellationTiming(),
            openEventStream: openerFactory.openEventStream)

        session.start()
        await openerFactory.waitForOpen()
        session.stop()
        try await openerFactory.releaseOpen(ordinal: 1)

        XCTAssertNil(session.conversation)
        XCTAssertEqual(session.connection, .idle)
    }

    @MainActor
    func testAPIReplacementMakesOldLateStreamIgnored() async throws {
        let oldStream = AsyncThrowingStream<PhoenixEvent, Error> { continuation in
            continuation.yield(.initSnapshot(.init(
                conversation: try! self.conversation(id: "c1", aggregateId: "pc-old"),
                messages: [try! self.message(id: "m-old", content: "[{\"type\":\"text\",\"text\":\"stale\"}]")],
                agentWorking: false,
                presentationMode: "idle", lastSequenceId: 1,
                pendingAnchorSequenceId: 1, pendingEvents: [], pendingTruncated: false)))
            continuation.finish()
        }
        let newStream = AsyncThrowingStream<PhoenixEvent, Error> { continuation in
            continuation.yield(.initSnapshot(.init(
                conversation: try! self.conversation(id: "c1", aggregateId: "pc-new"),
                messages: [try! self.message(id: "m-new", content: "[{\"type\":\"text\",\"text\":\"fresh\"}]")],
                agentWorking: false,
                presentationMode: "idle",
                lastSequenceId: 2,
                pendingAnchorSequenceId: 2,
                pendingEvents: [],
                pendingTruncated: false)))
            continuation.finish()
        }
        let openerFactory = ScriptedStreamOpeningFactory(
            steps: [oldStream, newStream],
            blockedOrdinals: [],
            blockedIgnoringCancellationOrdinals: [1])
        let conversationGate = ConversationUpdateGate()
        let session = makeSession(
            onConversationUpdate: { conversation in
                conversationGate.observe(conversation)
            },
            retryTiming: ImmediateCancellationTiming(),
            staleCheckTiming: ImmediateCancellationTiming(),
            openEventStream: openerFactory.openEventStream)

        defer {
            openerFactory.releaseAllNonCooperativeWaiters()
            conversationGate.releaseAll()
        }

        session.start()
        await openerFactory.waitForOpen(count: 1)
        guard let oldTask = session.currentStreamTaskForTesting() else {
            XCTFail("expected old stream task")
            return
        }
        let originalIdentity = openerFactory.recordedAPIIdentities().first
        let replacement = PhoenixAPI(baseURL: URL(string: "https://phoenix.example")!, password: nil, allowSelfSigned: false, configurationIdentity: APIConfigurationIdentity(serverURL: "https://phoenix.example", credentialGeneration: "phoenix.example", trustSelfSigned: false))!
        session.replaceAPI(replacement)
        await openerFactory.waitForOpen(count: 2)
        await conversationGate.wait()

        let recordedIdentities = openerFactory.recordedAPIIdentities()
        XCTAssertEqual(recordedIdentities.count, 2)
        XCTAssertNotEqual(recordedIdentities[0], recordedIdentities[1])
        XCTAssertEqual(recordedIdentities.first, originalIdentity)
        XCTAssertEqual(recordedIdentities[1], replacement.configurationIdentity)
        let connection = session.connection
        XCTAssertEqual(connection, ConversationSession.ConnectionState.live)
        let conversationBeforeOldRelease = session.conversation
        XCTAssertEqual(conversationBeforeOldRelease?.product_conversation_id, "pc-new")
        let firstMessageBeforeOldRelease = session.messages.first
        XCTAssertEqual(firstMessageBeforeOldRelease?.message_id, "m-new")
        XCTAssertEqual(firstMessageBeforeOldRelease?.content.arrayValue?.first?["text"]?.stringValue, "fresh")

        try await openerFactory.releaseOpen(ordinal: 1)
        await oldTask.value

        let finalConversation = session.conversation
        XCTAssertEqual(finalConversation?.product_conversation_id, "pc-new")
        let finalFirstMessage = session.messages.first
        XCTAssertEqual(finalFirstMessage?.message_id, "m-new")
        XCTAssertEqual(finalFirstMessage?.content.arrayValue?.first?["text"]?.stringValue, "fresh")
        XCTAssertNotEqual(finalConversation?.product_conversation_id, "pc-old")
        XCTAssertNotEqual(finalFirstMessage?.message_id, "m-old")
    }

    @MainActor
    func testStaleCheckSleepReleasedAfterStopDoesNotSurfaceEntries() async throws {
        let stream = AsyncThrowingStream<PhoenixEvent, Error> { continuation in continuation.finish() }
        let staleCheckTiming = ControlledTiming()
        let session = makeSession(
            retryTiming: ImmediateCancellationTiming(),
            staleCheckTiming: staleCheckTiming,
            openEventStream: ScriptedStreamOpeningFactory(steps: [stream], blockedOrdinals: []).openEventStream)
        let entry = await session.outbox.enqueue(text: "hi")!
        session.outbox.markAccepted(entry.localId, steering: false)

        session.start()
        session.receive(.stateChange(seq: 1, state: .string("working"), presentationMode: "working", stateUpdatedAt: nil))
        await staleCheckTiming.waitForSleepEntry()
        session.stop()
        try await staleCheckTiming.releaseSleep()

        let status = session.outbox.entries.first?.status
        XCTAssertEqual(status, .pending)
    }

    @MainActor
    func testCancelledRetrySleepDoesNotAdvanceBackoffOrReopen() async throws {
        let stream = AsyncThrowingStream<PhoenixEvent, Error> { continuation in
            continuation.finish(throwing: APIError.transport(underlying: URLError(.networkConnectionLost)))
        }
        let retryTiming = ControlledTiming()
        let openerFactory = ScriptedStreamOpeningFactory(steps: [stream], blockedOrdinals: [])
        let session = makeSession(
            retryTiming: retryTiming,
            staleCheckTiming: ImmediateCancellationTiming(),
            openEventStream: openerFactory.openEventStream)

        session.start()
        await openerFactory.waitForOpen()
        await retryTiming.waitForSleepEntry()
        session.stop()
        try await retryTiming.releaseSleep()

        let connection = session.connection
        if case .waitingToRetry = connection {
            XCTFail("canceled retry sleep should not leave waiting-to-retry state")
        }
    }
}
