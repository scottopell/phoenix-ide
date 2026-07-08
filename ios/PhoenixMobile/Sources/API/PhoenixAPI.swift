import CryptoKit
import Foundation
import Security

enum APIError: Error, LocalizedError {
    /// Network-level failure — no HTTP response was received. Retryable:
    /// the request may never have reached the server, and even if it did,
    /// resends are safe because every chat POST carries an idempotent
    /// `message_id`.
    case transport(underlying: Error)
    /// The server answered with a non-2xx status. Not silently retryable —
    /// the server saw the request and rejected it.
    case http(status: Int, body: String)
    case decoding(underlying: Error)
    case invalidURL

    var errorDescription: String? {
        switch self {
        case .transport(let e): return "Network error: \(e.localizedDescription)"
        case .http(let status, let body):
            let detail = body.prefix(200)
            return detail.isEmpty ? "Server error (HTTP \(status))" : "HTTP \(status): \(detail)"
        case .decoding(let e): return "Unexpected server response: \(e.localizedDescription)"
        case .invalidURL: return "Invalid server URL"
        }
    }

    var isTransport: Bool {
        if case .transport = self { return true }
        return false
    }
}

/// Server trust handling for Phoenix's self-signed TLS posture (TLS.md):
/// CA-valid certificates pass standard evaluation untouched; when the user
/// enabled self-signed trust, a failing chain is accepted only under the
/// trust-on-first-use pin in CertPinStore (REQ-IOS-008) — never blindly,
/// because every request carries the Bearer password.
final class ServerTrustDelegate: NSObject, URLSessionDelegate {
    let allowSelfSigned: Bool

    init(allowSelfSigned: Bool) {
        self.allowSelfSigned = allowSelfSigned
    }

    func urlSession(
        _ session: URLSession,
        didReceive challenge: URLAuthenticationChallenge,
        completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
    ) {
        guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
              let trust = challenge.protectionSpace.serverTrust
        else {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        // Properly CA-signed certificates need no pinning.
        if SecTrustEvaluateWithError(trust, nil) {
            completionHandler(.useCredential, URLCredential(trust: trust))
            return
        }
        guard allowSelfSigned, let fingerprint = Self.leafFingerprint(trust) else {
            completionHandler(.performDefaultHandling, nil)
            return
        }
        let space = challenge.protectionSpace
        switch CertPinStore.evaluate(host: space.host, port: space.port, fingerprint: fingerprint) {
        case .accept:
            completionHandler(.useCredential, URLCredential(trust: trust))
        case .reject:
            // Pin mismatch: fail closed. Settings shows the mismatch and
            // offers an explicit "forget pin" re-trust path.
            completionHandler(.cancelAuthenticationChallenge, nil)
        }
    }

    /// SHA-256 over the leaf certificate's DER bytes, lowercase hex.
    private static func leafFingerprint(_ trust: SecTrust) -> String? {
        guard let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
              let leaf = chain.first
        else { return nil }
        let der = SecCertificateCopyData(leaf) as Data
        return SHA256.hash(data: der).map { String(format: "%02x", $0) }.joined()
    }
}

/// REST client for the Phoenix API. Non-browser clients authenticate with
/// `Authorization: Bearer <password>`; the phoenix-auth cookie is a
/// browser-only session token, not a client credential.
struct PhoenixAPI: Sendable {
    let baseURL: URL
    let password: String?
    private let session: URLSession
    /// Long-lived session for SSE: effectively no per-request deadline; the
    /// idle timeout covers gaps between events (the server keep-alives).
    private let streamSession: URLSession

    init(baseURL: URL, password: String?, allowSelfSigned: Bool) {
        self.baseURL = baseURL
        self.password = password

        let delegate = ServerTrustDelegate(allowSelfSigned: allowSelfSigned)

        let config = URLSessionConfiguration.default
        config.timeoutIntervalForRequest = 30
        config.waitsForConnectivity = false
        self.session = URLSession(configuration: config, delegate: delegate, delegateQueue: nil)

        let streamConfig = URLSessionConfiguration.default
        // timeoutIntervalForRequest is an idle timeout for streaming bodies
        // (it resets whenever bytes arrive). The server sends SSE keep-alive
        // comments, so 90s of silence means the connection is dead.
        streamConfig.timeoutIntervalForRequest = 90
        streamConfig.timeoutIntervalForResource = .infinity
        streamConfig.waitsForConnectivity = false
        self.streamSession = URLSession(
            configuration: streamConfig, delegate: delegate, delegateQueue: nil)
    }

    private func request(path: String, query: [URLQueryItem] = []) throws -> URLRequest {
        guard var components = URLComponents(
            url: baseURL.appendingPathComponent(path), resolvingAgainstBaseURL: false)
        else { throw APIError.invalidURL }
        if !query.isEmpty { components.queryItems = query }
        guard let url = components.url else { throw APIError.invalidURL }
        var req = URLRequest(url: url)
        if let password, !password.isEmpty {
            req.setValue("Bearer \(password)", forHTTPHeaderField: "Authorization")
        }
        return req
    }

    private func send<T: Decodable>(_ req: URLRequest, as type: T.Type) async throws -> T {
        let data: Data
        let response: URLResponse
        do {
            (data, response) = try await session.data(for: req)
        } catch {
            throw APIError.transport(underlying: error)
        }
        guard let http = response as? HTTPURLResponse else {
            throw APIError.transport(underlying: URLError(.badServerResponse))
        }
        guard (200..<300).contains(http.statusCode) else {
            throw APIError.http(
                status: http.statusCode,
                body: String(data: data, encoding: .utf8) ?? "")
        }
        do {
            return try JSONDecoder().decode(type, from: data)
        } catch {
            throw APIError.decoding(underlying: error)
        }
    }

    private func get<T: Decodable>(
        _ path: String, query: [URLQueryItem] = [], as type: T.Type
    ) async throws -> T {
        try await send(request(path: path, query: query), as: type)
    }

    private func post<T: Decodable>(
        _ path: String, body: [String: Any], as type: T.Type
    ) async throws -> T {
        var req = try request(path: path)
        req.httpMethod = "POST"
        req.setValue("application/json", forHTTPHeaderField: "Content-Type")
        req.httpBody = try JSONSerialization.data(withJSONObject: body)
        return try await send(req, as: type)
    }

    // MARK: - Endpoints

    func authStatus() async throws -> AuthStatusResponse {
        try await get("api/auth/status", as: AuthStatusResponse.self)
    }

    func listConversations() async throws -> [Conversation] {
        try await get("api/conversations", as: ConversationListResponse.self).conversations
    }

    func getConversation(id: String, afterSequence: Int64 = 0) async throws
        -> ConversationWithMessagesResponse
    {
        var query: [URLQueryItem] = []
        if afterSequence > 0 {
            query.append(URLQueryItem(name: "after_sequence", value: String(afterSequence)))
        }
        return try await get(
            "api/conversations/\(id)", query: query,
            as: ConversationWithMessagesResponse.self)
    }

    /// Idempotent by `messageId`: the server returns success without a
    /// duplicate when the same id is resent, which makes offline retries safe.
    func sendChat(conversationId: String, text: String, images: [ImagePayload], messageId: String)
        async throws -> ChatResponse
    {
        try await post(
            "api/conversations/\(conversationId)/chat",
            body: [
                "text": text,
                "message_id": messageId,
                "images": images.map { ["data": $0.data, "media_type": $0.media_type] },
                "user_agent": "PhoenixMobile-iOS",
            ],
            as: ChatResponse.self)
    }

    func createConversation(cwd: String, text: String, model: String?, messageId: String)
        async throws -> Conversation
    {
        var body: [String: Any] = [
            "cwd": cwd,
            "text": text,
            "images": [] as [[String: String]],
            "message_id": messageId,
        ]
        if let model { body["model"] = model }
        return try await post("api/conversations/new", body: body, as: ConversationResponse.self)
            .conversation
    }

    func cancel(conversationId: String) async throws -> CancelResponse {
        try await post(
            "api/conversations/\(conversationId)/cancel", body: [:], as: CancelResponse.self)
    }

    func archive(conversationId: String) async throws {
        struct OkResponse: Codable { var ok: Bool? }
        _ = try await post(
            "api/conversations/\(conversationId)/archive", body: [:], as: OkResponse.self)
    }

    /// Clears a user-resumable error state. The server responds 409 when
    /// the error is not dismissable or the conversation isn't errored.
    func dismissError(conversationId: String) async throws {
        struct SuccessResponse: Codable { var success: Bool? }
        _ = try await post(
            "api/conversations/\(conversationId)/dismiss-error", body: [:],
            as: SuccessResponse.self)
    }

    func validateCwd(path: String) async throws -> ValidateCwdResponse {
        try await get(
            "api/validate-cwd",
            query: [URLQueryItem(name: "path", value: path)],
            as: ValidateCwdResponse.self)
    }

    func models() async throws -> ModelsResponse {
        try await get("api/models", as: ModelsResponse.self)
    }

    // MARK: - SSE

    /// Open the conversation event stream. The caller consumes raw bytes via
    /// SSEParser; each (re)connect delivers a fresh `init` snapshot including
    /// the server's replay ring, so no separate `after` bookkeeping is needed.
    func openStream(conversationId: String) async throws -> (URLSession.AsyncBytes, HTTPURLResponse) {
        var req = try request(path: "api/conversations/\(conversationId)/stream")
        req.setValue("text/event-stream", forHTTPHeaderField: "Accept")
        req.timeoutInterval = 90
        let (bytes, response): (URLSession.AsyncBytes, URLResponse)
        do {
            (bytes, response) = try await streamSession.bytes(for: req)
        } catch {
            throw APIError.transport(underlying: error)
        }
        guard let http = response as? HTTPURLResponse else {
            throw APIError.transport(underlying: URLError(.badServerResponse))
        }
        guard http.statusCode == 200 else {
            throw APIError.http(status: http.statusCode, body: "")
        }
        return (bytes, http)
    }
}
