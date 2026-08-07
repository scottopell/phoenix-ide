import Foundation
import Combine
import Darwin

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
    case unavailable(String)
    case tlsFailure(String)
    case wrongService(String)
    case unsupportedOwnership(String)
    case failed(String)

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
        case .unsupportedOwnership: "Unsupported deployment"
        case .failed: "Failed"
        }
    }

    var canDisplayWebView: Bool {
        switch self {
        case .identityVerified, .authenticationRequired, .verifyingDeployment, .ready: true
        default: false
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
    let processID: Int32?
    let executablePath: String?
    let databasePath: String?
    let logPath: String?
    let recentLogLines: [String]
}

@MainActor
final class ServerManager: ObservableObject {
    @Published private(set) var state: ConnectionState = .stopped
    @Published private(set) var mode: ServerMode?
    @Published private(set) var webOrigin: PhoenixOrigin?

    private var process: Process?
    private var readinessTask: Task<Void, Never>?
    private var stopDeadline: DispatchSourceTimer?
    private var stopCompletions: [() -> Void] = []
    private var logFileHandle: FileHandle?
    private var recentLines: [String] = []
    private var ownerLockDescriptor: Int32?
    private var operationID = UUID()
    private let keychain = KeychainStore()

    init() {
        ConfigurationStore.removeLegacyPlaintextSecret()
    }

    func connect() {
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
                    await self?.verifyIdentity(for: selected, operation: operation)
                }
            case .bundled(let configuration):
                try startBundled(configuration, operation: operation)
            }
        } catch {
            state = .failed(error.localizedDescription)
        }
    }

    func reconnect() {
        switch mode {
        case .bundled?: restartBundled()
        default: connect()
        }
    }

    func deploymentReceived(_ deployment: DeploymentInfo) {
        guard let selected = mode else { return }
        guard let version = currentVersion else { return }
        state = .verifyingDeployment(version)
        if let violation = deploymentViolation(deployment, for: selected) {
            state = .unsupportedOwnership(violation)
        } else {
            state = .ready(version, deployment)
        }
    }

    func deploymentRequiresAuthentication() {
        guard let version = currentVersion else { return }
        state = .authenticationRequired(version)
    }

    func deploymentVerificationFailed(_ message: String) {
        guard state.canDisplayWebView else { return }
        state = .unavailable(message)
    }

    func stop(completion: (() -> Void)? = nil) {
        if let completion { stopCompletions.append(completion) }
        readinessTask?.cancel()
        readinessTask = nil

        guard case .bundled = mode, let process, process.isRunning else {
            finishStop()
            return
        }
        state = .stopping
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
            processID: process?.processIdentifier,
            executablePath: bundled?.executableURL.path,
            databasePath: bundled?.databaseURL.path,
            logPath: bundled?.logURL.path,
            recentLogLines: recentLines
        )
    }

    private var currentVersion: VersionInfo? {
        switch state {
        case .identityVerified(let value), .authenticationRequired(let value), .verifyingDeployment(let value): value
        case .ready(let value, _): value
        default: nil
        }
    }

    private var currentDeployment: DeploymentInfo? {
        if case .ready(_, let deployment) = state { return deployment }
        return nil
    }

    private func startBundled(_ configuration: BundledServerConfiguration, operation: UUID) throws {
        try acquireOwnerLock(configuration.ownerLockURL)
        state = .startingSidecar
        recentLines = []

        try FileManager.default.createDirectory(
            at: configuration.databaseURL.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        FileManager.default.createFile(atPath: configuration.logURL.path, contents: nil)
        logFileHandle = try FileHandle(forWritingTo: configuration.logURL)
        try logFileHandle?.seekToEnd()

        let inherited = ProcessInfo.processInfo.environment
        var environment = [
            "HOME": FileManager.default.homeDirectoryForCurrentUser.path,
            "PATH": inherited["PATH"] ?? "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            "TMPDIR": inherited["TMPDIR"] ?? NSTemporaryDirectory(),
            "LANG": inherited["LANG"] ?? "en_US.UTF-8",
        ]
        for (key, value) in configuration.publicEnvironment { environment[key] = value }
        for (key, value) in keychain.processEnvironment() { environment[key] = value }

        let launched = Process()
        launched.executableURL = configuration.executableURL
        launched.environment = environment
        launched.currentDirectoryURL = FileManager.default.homeDirectoryForCurrentUser
        launched.qualityOfService = .userInitiated

        let pipe = Pipe()
        launched.standardOutput = pipe
        launched.standardError = pipe
        pipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            guard !data.isEmpty else { return }
            Task { @MainActor in self?.recordOutput(data) }
        }
        launched.terminationHandler = { [weak self] terminated in
            Task { @MainActor in self?.processExited(terminated, operation: operation) }
        }

        do {
            try launched.run()
        } catch {
            try? logFileHandle?.close()
            logFileHandle = nil
            releaseOwnerLock()
            throw error
        }
        process = launched
        readinessTask = Task { [weak self] in
            await self?.waitForBundledIdentity(configuration, operation: operation)
        }
    }

    private func restartBundled() {
        guard case .bundled = mode else { return }
        state = .restarting
        stop { [weak self] in self?.connect() }
    }

    private func verifyIdentity(for selected: ServerMode, operation: UUID) async {
        do {
            let version: VersionInfo = try await requestJSON(selected.origin.url(path: "/api/version"))
            guard operation == operationID else { return }
            state = .identityVerified(version)
            // Attached deployments complete verification in the WebView so its
            // Phoenix login cookie remains the single authenticated session.
            if case .bundled = selected {
                let deployment: DeploymentInfo = try await requestJSON(selected.origin.url(path: "/api/deployment"))
                guard operation == operationID else { return }
                deploymentReceived(deployment)
            }
        } catch let error as URLError where error.code == .serverCertificateUntrusted
            || error.code == .secureConnectionFailed
            || error.code == .clientCertificateRejected {
            state = .tlsFailure(error.localizedDescription)
        } catch let error as DecodingError {
            state = .wrongService(String(describing: error))
        } catch {
            state = .unavailable(error.localizedDescription)
        }
    }

    private func waitForBundledIdentity(_ configuration: BundledServerConfiguration, operation: UUID) async {
        let deadline = ContinuousClock.now + .seconds(30)
        while !Task.isCancelled && ContinuousClock.now < deadline {
            await verifyIdentity(for: .bundled(configuration), operation: operation)
            if state.canDisplayWebView { return }
            try? await Task.sleep(for: .milliseconds(300))
        }
        guard operation == operationID, process?.isRunning == true else { return }
        state = .failed("Bundled Phoenix did not become ready. Open Connection Status to locate the app-owned log.")
        stop()
    }

    private func requestJSON<T: Decodable>(_ url: URL) async throws -> T {
        var request = URLRequest(url: url)
        request.timeoutInterval = 5
        let (data, response) = try await URLSession.shared.data(for: request)
        guard let http = response as? HTTPURLResponse, http.statusCode == 200 else {
            throw URLError(.badServerResponse)
        }
        return try JSONDecoder().decode(T.self, from: data)
    }

    private func deploymentViolation(_ deployment: DeploymentInfo, for selected: ServerMode) -> String? {
        switch selected {
        case .attached:
            guard deployment.installationOwnership.grantsManagedAuthority else {
                return "This Phoenix is \(deployment.installationOwnership.label). It may be viewed, but Phoenix.app will not manage it."
            }
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
        }
        return nil
    }

    private func processExited(_ terminated: Process, operation: UUID) {
        guard process === terminated else { return }
        stopDeadline?.cancel()
        stopDeadline = nil
        process = nil
        releaseOwnerLock()
        try? logFileHandle?.close()
        logFileHandle = nil

        switch state {
        case .stopping, .restarting:
            finishStop()
        default:
            if operation == operationID {
                state = .failed("Bundled Phoenix exited with code \(terminated.terminationStatus). Open Connection Status to locate the app-owned log.")
            }
            finishStopCallbacksOnly()
        }
    }

    private func finishStop() {
        stopDeadline?.cancel()
        stopDeadline = nil
        process = nil
        releaseOwnerLock()
        state = .stopped
        finishStopCallbacksOnly()
    }

    private func finishStopCallbacksOnly() {
        let callbacks = stopCompletions
        stopCompletions.removeAll()
        callbacks.forEach { $0() }
    }

    private func recordOutput(_ data: Data) {
        guard let text = String(data: data, encoding: .utf8) else { return }
        let sanitized = text.split(whereSeparator: \.isNewline).map { redact(String($0)) }
        if !sanitized.isEmpty {
            try? logFileHandle?.write(contentsOf: Data((sanitized.joined(separator: "\n") + "\n").utf8))
        }
        recentLines.append(contentsOf: sanitized)
        recentLines = Array(recentLines.suffix(80))
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
