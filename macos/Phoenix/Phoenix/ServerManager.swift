import Foundation
import Combine
import Darwin

struct FailureState: Equatable {
    let version: VersionInfo?
    let message: String
}

struct ConnectionLogBuffer: Equatable {
    private(set) var completeLines: [String] = []
    private(set) var pendingBytes = Data()
    let maxLines: Int

    init(maxLines: Int = 80) {
        self.maxLines = maxLines
    }

    var pendingFragment: String {
        String(decoding: pendingBytes, as: UTF8.self)
    }

    mutating func append(_ data: Data, redact: (String) -> String) -> [String] {
        guard !data.isEmpty else { return [] }
        pendingBytes.append(data)

        var emitted: [String] = []
        while let newlineIndex = pendingBytes.firstIndex(of: 0x0A) {
            let lineData = pendingBytes.prefix(upTo: newlineIndex)
            pendingBytes.removeSubrange(...newlineIndex)

            var normalized = Data(lineData)
            if normalized.last == 0x0D {
                normalized.removeLast()
            }
            emitted.append(redact(String(decoding: normalized, as: UTF8.self)))
        }

        if !emitted.isEmpty {
            completeLines.append(contentsOf: emitted)
            if completeLines.count > maxLines {
                completeLines = Array(completeLines.suffix(maxLines))
            }
        }
        return emitted
    }

    mutating func flushPending(redact: (String) -> String) -> String? {
        guard !pendingBytes.isEmpty else { return nil }
        let redacted = redact(String(decoding: pendingBytes, as: UTF8.self))
        pendingBytes.removeAll(keepingCapacity: true)
        completeLines.append(redacted)
        if completeLines.count > maxLines {
            completeLines = Array(completeLines.suffix(maxLines))
        }
        return redacted
    }
}

enum ServerIdentityError: Error, Equatable {
    case tls(String)
    case wrongService(String)
    case unavailable(String)
    case redirected(String)
}

struct IdentityHTTPStatusError: LocalizedError, Equatable {
    let statusCode: Int

    var errorDescription: String? { "Phoenix identity endpoint returned HTTP \(statusCode)." }
}

func classifyServerIdentityError(_ error: Error) -> ServerIdentityError {
    if let identity = error as? ServerIdentityError {
        return identity
    }
    if let urlError = error as? URLError, isCertificateURLError(urlError.code) {
        return .tls(urlError.localizedDescription)
    }
    if let decoding = error as? DecodingError {
        return .wrongService(String(describing: decoding))
    }
    if let status = error as? IdentityHTTPStatusError,
       status.statusCode == 404 || status.statusCode == 405 {
        return .wrongService(status.localizedDescription)
    }
    return .unavailable(error.localizedDescription)
}

final class RedirectRejectingURLSessionDelegate: NSObject, URLSessionTaskDelegate {
    private let expectedOrigin: PhoenixOrigin

    init(expectedOrigin: PhoenixOrigin) {
        self.expectedOrigin = expectedOrigin
    }

    func urlSession(
        _ session: URLSession,
        task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        guard let redirectedURL = request.url else {
            completionHandler(nil)
            return
        }
        if expectedOrigin.exactlyMatches(redirectedURL) {
            completionHandler(request)
        } else {
            completionHandler(nil)
        }
    }
}

func isCertificateURLError(_ code: URLError.Code) -> Bool {
    switch code {
    case .secureConnectionFailed,
         .serverCertificateHasBadDate,
         .serverCertificateUntrusted,
         .serverCertificateHasUnknownRoot,
         .serverCertificateNotYetValid,
         .clientCertificateRejected,
         .clientCertificateRequired:
        true
    default:
        false
    }
}

@MainActor
struct ConnectionErrorViewModel: Equatable {
    let message: String
    let allowsReconnect: Bool
}

@MainActor
enum ConnectionState: Equatable {
    case stopped
    case resolving
    case startingSidecar
    case identityVerified(VersionInfo)
    case authenticationRequired(VersionInfo)
    case verifyingDeployment(VersionInfo)
    case ready(VersionInfo, DeploymentInfo)
    case stopping
    case restarting
    case unavailable(FailureState)
    case tlsFailure(FailureState)
    case wrongService(FailureState)
    case unsupportedOwnership(VersionInfo, DeploymentInfo, String)
    case failed(FailureState)

    var displayName: String {
        switch self {
        case .stopped: "Stopped"
        case .resolving: "Connecting"
        case .startingSidecar: "Starting bundled Phoenix"
        case .identityVerified: "Phoenix found"
        case .authenticationRequired: "Sign in required"
        case .verifyingDeployment: "Verifying deployment"
        case .ready: "Ready"
        case .stopping: "Stopping"
        case .restarting: "Restarting"
        case .unavailable: "Unavailable"
        case .tlsFailure: "TLS trust failure"
        case .wrongService: "Wrong service"
        case .unsupportedOwnership: "Read-only deployment"
        case .failed: "Failed"
        }
    }

    var canDisplayWebView: Bool {
        switch self {
        case .identityVerified, .authenticationRequired, .verifyingDeployment, .ready, .unsupportedOwnership: true
        default: false
        }
    }

    var failureViewModel: ConnectionErrorViewModel? {
        switch self {
        case .failed(let failure), .unavailable(let failure), .tlsFailure(let failure), .wrongService(let failure):
            return ConnectionErrorViewModel(message: failure.message, allowsReconnect: true)
        case .unsupportedOwnership(_, _, let message):
            return ConnectionErrorViewModel(message: message, allowsReconnect: false)
        default:
            return nil
        }
    }

    var versionInfo: VersionInfo? {
        switch self {
        case .identityVerified(let value), .authenticationRequired(let value), .verifyingDeployment(let value): value
        case .ready(let value, _): value
        case .unsupportedOwnership(let value, _, _): value
        case .failed(let failure), .unavailable(let failure), .tlsFailure(let failure), .wrongService(let failure): failure.version
        default: nil
        }
    }

    var deploymentInfo: DeploymentInfo? {
        switch self {
        case .ready(_, let deployment), .unsupportedOwnership(_, let deployment, _): deployment
        default: nil
        }
    }
}

struct ServerStatusSnapshot {
    let state: ConnectionState
    let mode: ServerModeKind?
    let origin: String?
    let version: String?
    let gitSHA: String?
    let ownership: String?
    let bindAddress: String?
    let tlsEnabled: Bool?
    let socketActivated: Bool?
    let localAccess: Bool?
    let instanceID: String?
    let processID: Int32?
    let executablePath: String?
    let databasePath: String?
    let logPath: String?
    let recentLogLines: [String]
}

private struct LaunchedBundledInstance {
    let configuration: BundledServerConfiguration
    let instanceID: UUID
}

func deploymentMatchesBundledInstance(_ deployment: DeploymentInfo?, instanceID: UUID?) -> Bool {
    guard let instanceID else { return true }
    return deployment?.instanceID == instanceID.uuidString
}

struct BundledReconnectQueue {
    private(set) var latestCandidate: ServerMode?
    private(set) var stopScheduled = false

    mutating func request(_ candidate: ServerMode) -> Bool {
        latestCandidate = candidate
        guard !stopScheduled else { return false }
        stopScheduled = true
        return true
    }

    mutating func takeAfterStop() -> ServerMode? {
        stopScheduled = false
        defer { latestCandidate = nil }
        return latestCandidate
    }
}

struct RollingLogWriter {
    let url: URL
    let maxBytes: Int64

    func openForAppend(fileManager: FileManager = .default) throws -> FileHandle {
        if !fileManager.fileExists(atPath: url.path) {
            fileManager.createFile(atPath: url.path, contents: nil)
        }
        let existing = (try? Data(contentsOf: url)) ?? Data()
        let bounded = boundedTail(of: existing)
        if bounded != existing {
            try bounded.write(to: url, options: .atomic)
        }
        let handle = try FileHandle(forWritingTo: url)
        try handle.seekToEnd()
        return handle
    }

    func append(_ data: Data) throws -> FileHandle? {
        guard !data.isEmpty else { return nil }
        let existing = (try? Data(contentsOf: url)) ?? Data()
        var combined = existing
        combined.append(data)
        let bounded = boundedTail(of: combined)
        try bounded.write(to: url, options: .atomic)
        let handle = try FileHandle(forWritingTo: url)
        try handle.seekToEnd()
        return handle
    }

    func rewriteBoundedTail(fileManager: FileManager = .default) throws -> Data {
        guard maxBytes > 0 else {
            return (try? Data(contentsOf: url)) ?? Data()
        }
        let existing = (try? Data(contentsOf: url)) ?? Data()
        return boundedTail(of: existing)
    }

    static func boundedTail(_ data: Data, maxBytes: Int64) -> Data {
        Self(url: URL(fileURLWithPath: "/dev/null"), maxBytes: maxBytes).boundedTail(of: data)
    }

    private func boundedTail(of data: Data) -> Data {
        guard maxBytes > 0, data.count > maxBytes else { return data }

        let recordBounds = LogRecordBounds(maxRecordBytes: Int(maxBytes))
        let newline = UInt8(0x0A)
        var lines: [Data] = []
        var start = data.startIndex
        while let newlineIndex = data[start...].firstIndex(of: newline) {
            let line = Data(data[start...newlineIndex])
            lines.append(recordBounds.boundedRecord(line))
            start = data.index(after: newlineIndex)
        }
        if start < data.endIndex {
            let fragment = Data(data[start...])
            lines.append(recordBounds.boundedRecord(fragment))
        }

        guard !lines.isEmpty else { return recordBounds.boundedRecord(data) }

        var kept: [Data] = []
        var total = 0
        for line in lines.reversed() {
            if kept.isEmpty {
                kept.append(line)
                total += line.count
                continue
            }
            guard total + line.count <= maxBytes else { break }
            kept.append(line)
            total += line.count
        }
        return kept.reversed().reduce(into: Data()) { $0.append($1) }
    }
}

@MainActor
final class ServerManager: ObservableObject {
    @Published private(set) var state: ConnectionState = .stopped
    @Published private(set) var mode: ServerMode?
    @Published private(set) var webOrigin: PhoenixOrigin?
    @Published private(set) var recentLogLines: [String] = []

    private var process: Process?
    private var readinessTask: Task<Void, Never>?
    private var stopDeadline: DispatchSourceTimer?
    private var stopCompletions: [() -> Void] = []
    private var bundledReconnectQueue = BundledReconnectQueue()
    private var launcherLogHandle: FileHandle?
    private var ownerLockDescriptor: Int32?
    struct ConnectionOperationToken: Equatable, Hashable {
        fileprivate let id: UUID
    }

    private var operationID = UUID()
    private var launchedBundledInstance: LaunchedBundledInstance?
    private var logBuffer = ConnectionLogBuffer()
    private let keychain: any PackagedSidecarSecretProvider

    init(keychain: any PackagedSidecarSecretProvider = KeychainStore()) {
        self.keychain = keychain
        ConfigurationStore.removeLegacyPlaintextSecret()
    }

    var currentOperationToken: ConnectionOperationToken { ConnectionOperationToken(id: operationID) }

    func connect() {
        resetBrowserOwnedStateForOperationTransition()
        readinessTask?.cancel()
        operationID = UUID()
        let operation = operationID
        do {
            let selected = try ConfigurationStore.load()
            mode = selected
            webOrigin = selected.origin
            switch selected {
            case .attached:
                state = .resolving
                readinessTask = Task { [weak self] in
                    await self?.verifyIdentity(for: selected, operation: operation, allowIntermediateFailureState: true)
                }
            case .bundled(let configuration):
                try startBundled(configuration, operation: operation)
            }
        } catch {
            state = .failed(FailureState(version: currentVersion, message: error.localizedDescription))
        }
    }

    func reconnect() {
        do {
            let candidate = try ConfigurationStore.load()
            try reconnect(to: candidate)
        } catch {
            state = .failed(FailureState(version: currentVersion, message: error.localizedDescription))
        }
    }

    func reconnect(to candidate: ServerMode) throws {
        let decision = ConnectionReapplyDecision.evaluate(currentMode: mode, currentState: state, candidate: candidate)
        guard decision.requiresReconnect else { return }
        switch mode {
        case .bundled?:
            transitionFromBundled(to: candidate)
        default:
            mode = candidate
            webOrigin = candidate.origin
            connect()
        }
    }

    func showFailure(message: String) {
        resetBrowserOwnedStateForOperationTransition()
        state = .failed(FailureState(version: currentVersion, message: message))
    }

    func deploymentReceived(_ deployment: DeploymentInfo, operation: ConnectionOperationToken? = nil) {
        guard operation.map({ $0.id == operationID }) ?? true else { return }
        guard let selected = mode else { return }
        guard let version = currentVersion else { return }
        state = .verifyingDeployment(version)
        if let violation = deploymentViolation(deployment, for: selected, version: version) {
            state = .unsupportedOwnership(version, deployment, violation)
        } else {
            state = .ready(version, deployment)
        }
    }

    func deploymentRequiresAuthentication(operation: ConnectionOperationToken? = nil) {
        guard operation.map({ $0.id == operationID }) ?? true else { return }
        guard let version = currentVersion else { return }
        state = .authenticationRequired(version)
    }

    func deploymentVerificationFailed(_ message: String, operation: ConnectionOperationToken? = nil) {
        guard operation.map({ $0.id == operationID }) ?? true else { return }
        guard state.canDisplayWebView else { return }
        state = .unavailable(FailureState(version: currentVersion, message: message))
    }

    func stop(completion: (() -> Void)? = nil, preservedState: ConnectionState? = nil) {
        if let completion { stopCompletions.append(completion) }
        readinessTask?.cancel()
        readinessTask = nil

        guard case .bundled = mode, let process else {
            finishStop(finalState: preservedState ?? .stopped)
            return
        }
        guard process.isRunning else {
            flushPendingLogFragment()
            closeLauncherLog()
            releaseOwnerLock()
            self.process = nil
            launchedBundledInstance = nil
            finishStop(finalState: preservedState ?? .stopped)
            return
        }
        state = preservedState ?? .stopping
        process.terminate()

        let timer = DispatchSource.makeTimerSource(queue: .main)
        timer.schedule(deadline: .now() + 35)
        timer.setEventHandler { [weak process] in
            guard let process, process.isRunning else { return }
            kill(process.processIdentifier, SIGKILL)
        }
        stopDeadline = timer
        timer.resume()
    }

    func statusSnapshot() -> ServerStatusSnapshot {
        let selected = mode
        let version = currentVersion
        let deployment = currentDeployment
        let bundled: BundledServerConfiguration?
        if case .bundled(let configuration) = selected { bundled = configuration } else { bundled = nil }
        return ServerStatusSnapshot(
            state: state,
            mode: selected?.kind,
            origin: selected?.origin.description,
            version: version?.version,
            gitSHA: version?.gitSHA,
            ownership: deployment?.installationOwnership.label,
            bindAddress: deployment?.network.bindAddress,
            tlsEnabled: deployment?.network.tls.enabled,
            socketActivated: deployment?.network.socketActivated,
            localAccess: deployment?.localAccess,
            instanceID: deployment?.instanceID ?? launchedBundledInstance?.instanceID.uuidString,
            processID: process?.processIdentifier,
            executablePath: bundled?.executableURL.path,
            databasePath: bundled?.databaseURL.path,
            logPath: bundled?.launcherLogURL.path,
            recentLogLines: recentLogLines
        )
    }

    private var currentVersion: VersionInfo? { state.versionInfo }

    private var currentDeployment: DeploymentInfo? { state.deploymentInfo }

    private func resetBrowserOwnedStateForOperationTransition() {
        recentLogLines = []
    }

    private func stopPreservingFailureState(_ failure: FailureState) {
        stop(completion: nil, preservedState: .failed(failure))
    }

    private func startBundled(_ configuration: BundledServerConfiguration, operation: UUID) throws {
        state = .startingSidecar
        logBuffer = ConnectionLogBuffer()
        recentLogLines = []

        do {
            try FileManager.default.createDirectory(at: configuration.runtimeRootURL, withIntermediateDirectories: true)
            try FileManager.default.createDirectory(at: configuration.dataDirectoryURL, withIntermediateDirectories: true)
            try acquireOwnerLock(configuration.ownerLockURL)
            launcherLogHandle = try RollingLogWriter(
                url: configuration.launcherLogURL,
                maxBytes: BundledServerConfiguration.launcherLogMaxBytes
            ).openForAppend()

            let inherited = ProcessInfo.processInfo.environment
            let instanceID = UUID()
            let environment = SidecarLaunchEnvironment.build(
                inherited: inherited,
                privateHome: configuration.runtimeRootURL,
                instanceID: instanceID,
                publicEnvironment: configuration.publicEnvironment,
                sidecarSecrets: try keychain.sidecarEnvironment()
            )

            let launched = Process()
            launched.executableURL = configuration.executableURL
            launched.environment = environment
            launched.currentDirectoryURL = configuration.runtimeRootURL
            launched.qualityOfService = .userInitiated

            let pipe = Pipe()
            launched.standardOutput = pipe
            launched.standardError = pipe
            pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
                let data = handle.availableData
                Task { @MainActor in
                    guard let self else { return }
                    if data.isEmpty {
                        self.flushPendingLogFragment()
                        handle.readabilityHandler = nil
                        return
                    }
                    self.recordOutput(data)
                }
            }
            launched.terminationHandler = { [weak self] terminated in
                Task { @MainActor in self?.processExited(terminated, operation: operation) }
            }

            try launched.run()
            process = launched
            launchedBundledInstance = LaunchedBundledInstance(configuration: configuration, instanceID: instanceID)
            readinessTask = Task { [weak self] in
                await self?.waitForBundledIdentity(operation: operation)
            }
        } catch {
            cleanupLaunchPreparationFailure()
            throw error
        }
    }

    private func transitionFromBundled(to candidate: ServerMode) {
        guard case .bundled = mode else { return }
        guard bundledReconnectQueue.request(candidate) else { return }
        state = .restarting
        stop { [weak self] in
            guard let self else { return }
            guard let latestCandidate = self.bundledReconnectQueue.takeAfterStop() else { return }
            self.mode = latestCandidate
            self.webOrigin = latestCandidate.origin
            self.connect()
        }
    }

    private func verifyIdentity(for selected: ServerMode, operation: UUID, allowIntermediateFailureState: Bool) async {
        do {
            let version: VersionInfo = try await requestJSON(selected.origin.url(path: "/api/version"), expectedOrigin: selected.origin)
            guard operation == operationID else { return }
            state = .identityVerified(version)
            if case .bundled = selected {
                let deployment: DeploymentInfo = try await requestJSON(selected.origin.url(path: "/api/deployment"), expectedOrigin: selected.origin)
                guard operation == operationID else { return }
                deploymentReceived(deployment, operation: ConnectionOperationToken(id: operation))
            }
        } catch {
            guard operation == operationID else { return }
            if allowIntermediateFailureState {
                applyIdentityFailure(classifyServerIdentityError(error), version: currentVersion)
            }
        }
    }

    private func applyIdentityFailure(_ error: ServerIdentityError, version: VersionInfo?) {
        let failure = FailureState(version: version, message: {
            switch error {
            case .tls(let message), .wrongService(let message), .unavailable(let message), .redirected(let message): return message
            }
        }())
        switch error {
        case .tls:
            state = .tlsFailure(failure)
        case .wrongService:
            state = .wrongService(failure)
        case .unavailable, .redirected:
            state = .unavailable(failure)
        }
    }

    private func waitForBundledIdentity(operation: UUID) async {
        let deadline = ContinuousClock.now + .seconds(30)
        while !Task.isCancelled && ContinuousClock.now < deadline {
            guard let launchedBundledInstance else { return }
            await verifyIdentity(for: .bundled(launchedBundledInstance.configuration), operation: operation, allowIntermediateFailureState: false)
            if case .ready(_, let deployment) = state,
               deployment.instanceID == launchedBundledInstance.instanceID.uuidString {
                return
            }
            guard operation == operationID else { return }
            guard process?.isRunning == true else { return }
            try? await Task.sleep(for: .milliseconds(300))
        }
        guard operation == operationID, process?.isRunning == true else { return }
        let failure = FailureState(version: currentVersion, message: "Bundled Phoenix did not become ready. Open Connection Status to locate the app-owned log.")
        state = .failed(failure)
        stopPreservingFailureState(failure)
    }

    private func requestJSON<T: Decodable>(_ url: URL, expectedOrigin: PhoenixOrigin) async throws -> T {
        var request = URLRequest(url: url)
        request.timeoutInterval = 5
        let delegate = RedirectRejectingURLSessionDelegate(expectedOrigin: expectedOrigin)
        let session = URLSession(configuration: .ephemeral, delegate: delegate, delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse else {
                throw URLError(.badServerResponse)
            }
            guard http.statusCode == 200 else {
                throw IdentityHTTPStatusError(statusCode: http.statusCode)
            }
            guard let finalURL = response.url, finalURL == url, expectedOrigin.exactlyMatches(finalURL) else {
                throw ServerIdentityError.redirected("Phoenix identity verification redirected away from the configured origin.")
            }
            return try JSONDecoder().decode(T.self, from: data)
        } catch let error as ServerIdentityError {
            throw error
        } catch let error as IdentityHTTPStatusError {
            throw error
        } catch let error as URLError where error.code == .badServerResponse {
            throw error
        } catch {
            if let nsError = error as NSError?, nsError.domain == NSURLErrorDomain, nsError.code == URLError.cancelled.rawValue {
                throw ServerIdentityError.redirected("Phoenix identity verification redirected away from the configured origin.")
            }
            throw error
        }
    }

    private func deploymentViolation(_ deployment: DeploymentInfo, for selected: ServerMode, version: VersionInfo) -> String? {
        guard deployment.build.version == version.version, deployment.build.gitSHA == version.gitSHA else {
            return "Phoenix reported conflicting build identity between /api/version and /api/deployment."
        }
        switch selected {
        case .attached:
            return nil
        case .bundled:
            let host = deployment.network.bindAddress.split(separator: ":").first.map(String.init) ?? ""
            guard host == "127.0.0.1" || host == "[::1]" else {
                return "Bundled Phoenix reported a non-loopback listener: \(deployment.network.bindAddress)"
            }
            guard !deployment.network.tls.enabled, !deployment.network.socketActivated else {
                return "Bundled Phoenix reported an unexpected TLS or socket-activation posture."
            }
            guard !deployment.installationOwnership.grantsManagedAuthority else {
                return "Bundled Phoenix unexpectedly claimed managed update authority."
            }
            guard deploymentMatchesLaunchedInstance(deployment) else {
                return "Bundled Phoenix responded from a different launch instance than the sidecar Phoenix.app started."
            }
            return nil
        }
    }

    private func processExited(_ terminated: Process, operation: UUID) {
        guard process === terminated else { return }
        let priorState = state
        let version = currentVersion
        flushPendingLogFragment()
        closeLauncherLog()
        stopDeadline?.cancel()
        stopDeadline = nil
        releaseOwnerLock()
        process = nil
        launchedBundledInstance = nil

        switch priorState {
        case .stopping, .restarting:
            finishStop(finalState: .stopped)
        case .failed(let failure) where operation == operationID:
            finishStop(finalState: .failed(failure))
        default:
            let finalState: ConnectionState
            if operation == operationID {
                finalState = .failed(FailureState(version: version, message: "Bundled Phoenix exited with code \(terminated.terminationStatus). Open Connection Status to locate the app-owned log."))
            } else {
                finalState = .stopped
            }
            finishStop(finalState: finalState)
        }
    }

    private func cleanupLaunchPreparationFailure() {
        launchedBundledInstance = nil
        closeLauncherLog()
        releaseOwnerLock()
    }

    private func deploymentMatchesLaunchedInstance(_ deployment: DeploymentInfo?) -> Bool {
        deploymentMatchesBundledInstance(deployment, instanceID: launchedBundledInstance?.instanceID)
    }

    private func finishStop(finalState: ConnectionState) {
        stopDeadline?.cancel()
        stopDeadline = nil
        process = nil
        launchedBundledInstance = nil
        releaseOwnerLock()
        closeLauncherLog()
        state = finalState
        finishStopCallbacksOnly()
    }


    private func finishStopCallbacksOnly() {
        let callbacks = stopCompletions
        stopCompletions.removeAll()
        callbacks.forEach { $0() }
    }

    private func recordOutput(_ data: Data) {
        let emitted = logBuffer.append(data, redact: redact)
        if !emitted.isEmpty {
            appendToLauncherLog(Data((emitted.joined(separator: "\n") + "\n").utf8))
            recentLogLines = logBuffer.completeLines
        }
    }

    private func flushPendingLogFragment() {
        guard let flushed = logBuffer.flushPending(redact: redact) else { return }
        appendToLauncherLog(Data((flushed + "\n").utf8))
        recentLogLines = logBuffer.completeLines
    }

    private func appendToLauncherLog(_ data: Data) {
        guard let configuration = launchedBundledInstance?.configuration else { return }
        let writer = RollingLogWriter(
            url: configuration.launcherLogURL,
            maxBytes: BundledServerConfiguration.launcherLogMaxBytes
        )
        do {
            try launcherLogHandle?.close()
            launcherLogHandle = try writer.append(data)
        } catch {
            launcherLogHandle = nil
        }
    }

    private func redact(_ line: String) -> String {
        let sensitiveNames = [
            "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "PHOENIX_PASSWORD",
            "authorization", "bearer", "token", "api_key", "apikey",
        ]
        let lowercased = line.lowercased()
        guard sensitiveNames.contains(where: { lowercased.contains($0.lowercased()) }) else { return line }
        if let separator = line.firstIndex(where: { $0 == "=" || $0 == ":" }) {
            return String(line[...separator]) + " [REDACTED]"
        }
        return "[REDACTED sensitive log line]"
    }

    private func closeLauncherLog() {
        try? launcherLogHandle?.close()
        launcherLogHandle = nil
    }

    private func acquireOwnerLock(_ url: URL) throws {
        let descriptor = open(url.path, O_RDWR | O_CREAT, S_IRUSR | S_IWUSR)
        guard descriptor >= 0 else { throw ConfigurationError.bundledDataInUse }
        guard flock(descriptor, LOCK_EX | LOCK_NB) == 0 else {
            close(descriptor)
            throw ConfigurationError.bundledDataInUse
        }
        ftruncate(descriptor, 0)
        let bytes = Array("\(ProcessInfo.processInfo.processIdentifier)\n".utf8)
        _ = bytes.withUnsafeBytes { write(descriptor, $0.baseAddress, $0.count) }
        ownerLockDescriptor = descriptor
    }

    private func releaseOwnerLock() {
        guard let descriptor = ownerLockDescriptor else { return }
        flock(descriptor, LOCK_UN)
        close(descriptor)
        ownerLockDescriptor = nil
    }
}
