import Foundation
import Darwin

enum CommandExecutableResolution: Equatable, Sendable {
    case empty
    case shellBuiltin(String)
    case found(executable: String, resolvedPath: String)
    case notFound(executable: String)
}

enum LoginShellPathResolver {
    private static let sentinelBegin = "__PHOENIX_PATH_BEGIN__"
    private static let sentinelEnd = "__PHOENIX_PATH_END__"
    private static let shellBuiltins: Set<String> = [
        "echo", "printf", "export", "eval", "exec", "source", ".", "true", "false", "test", "[", ":",
    ]

    static func defaultFallbackPaths(userHome: URL) -> [String] {
        [
            userHome.appendingPathComponent(".local/bin").path,
            userHome.appendingPathComponent("bin").path,
            "/opt/homebrew/bin",
            "/usr/local/bin",
            "/usr/bin",
            "/bin",
            "/usr/sbin",
            "/sbin",
        ]
    }

    static func mergedSearchPath(
        loginShellPath: String?,
        inheritedPath: String?,
        userHome: URL = FileManager.default.homeDirectoryForCurrentUser
    ) -> String {
        var entries: [String] = []
        var seen = Set<String>()
        func appendEntries(from raw: String?) {
            guard let raw, !raw.isEmpty else { return }
            for segment in raw.split(separator: ":").map(String.init) where !segment.isEmpty {
                if seen.insert(segment).inserted {
                    entries.append(segment)
                }
            }
        }
        appendEntries(from: loginShellPath)
        appendEntries(from: inheritedPath)
        for fallback in defaultFallbackPaths(userHome: userHome) where seen.insert(fallback).inserted {
            entries.append(fallback)
        }
        return entries.joined(separator: ":")
    }

    static func extractSentinelPath(from output: String) -> String? {
        guard let beginRange = output.range(of: sentinelBegin, options: .backwards) else {
            return nil
        }
        let remainder = output[beginRange.upperBound...]
        guard let endRange = remainder.range(of: sentinelEnd) else {
            return nil
        }
        let extracted = String(remainder[..<endRange.lowerBound])
            .trimmingCharacters(in: .whitespacesAndNewlines)
        return extracted.isEmpty ? nil : extracted
    }

    static func captureLoginShellPath(
        inherited: [String: String] = ProcessInfo.processInfo.environment,
        userHome: URL = FileManager.default.homeDirectoryForCurrentUser,
        timeoutSeconds: TimeInterval = 2.0
    ) -> String? {
        let candidateShell = inherited["SHELL"].flatMap { shell -> String? in
            guard shell.hasPrefix("/"), FileManager.default.isExecutableFile(atPath: shell) else {
                return nil
            }
            return shell
        } ?? "/bin/zsh"

        guard FileManager.default.isExecutableFile(atPath: candidateShell) else {
            return nil
        }

        let process = Process()
        process.executableURL = URL(fileURLWithPath: candidateShell)
        process.arguments = [
            "-l", "-c",
            "printf '\(sentinelBegin)%s\(sentinelEnd)' \"$PATH\"",
        ]
        var probeEnv: [String: String] = [
            "HOME": userHome.path,
            "TERM": "dumb",
            "PATH": inherited["PATH"] ?? "/usr/bin:/bin:/usr/sbin:/sbin",
        ]
        for key in ["USER", "LOGNAME", "LANG"] {
            if let value = inherited[key], !value.isEmpty {
                probeEnv[key] = value
            }
        }
        process.environment = probeEnv
        process.standardInput = FileHandle.nullDevice
        let stdoutPipe = Pipe()
        process.standardOutput = stdoutPipe
        process.standardError = FileHandle.nullDevice

        final class OutputBox: @unchecked Sendable {
            let lock = NSLock()
            var data = Data()
            func append(_ chunk: Data) {
                lock.lock()
                data.append(chunk)
                lock.unlock()
            }
            func snapshot() -> Data {
                lock.lock()
                defer { lock.unlock() }
                return data
            }
        }

        let box = OutputBox()
        let readGroup = DispatchGroup()
        readGroup.enter()
        DispatchQueue.global(qos: .utility).async {
            let data = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
            box.append(data)
            readGroup.leave()
        }

        let exitSemaphore = DispatchSemaphore(value: 0)
        process.terminationHandler = { _ in
            exitSemaphore.signal()
        }

        do {
            try process.run()
        } catch {
            stdoutPipe.fileHandleForReading.closeFile()
            return nil
        }

        let timedOut = exitSemaphore.wait(timeout: .now() + max(0.2, timeoutSeconds)) == .timedOut
        if timedOut {
            if process.isRunning {
                process.terminate()
                if exitSemaphore.wait(timeout: .now() + 0.3) == .timedOut, process.isRunning {
                    kill(process.processIdentifier, SIGKILL)
                    _ = exitSemaphore.wait(timeout: .now() + 0.3)
                }
            }
            stdoutPipe.fileHandleForReading.closeFile()
            return nil
        }

        _ = readGroup.wait(timeout: .now() + 0.5)
        guard process.terminationStatus == 0 else { return nil }
        let output = String(decoding: box.snapshot(), as: UTF8.self)
        return extractSentinelPath(from: output)
    }

    static func resolveCommandExecutable(
        _ command: String,
        searchPath: String,
        userHome: URL = FileManager.default.homeDirectoryForCurrentUser,
        fileExistsExecutable: (String) -> Bool = { FileManager.default.isExecutableFile(atPath: $0) }
    ) -> CommandExecutableResolution {
        let tokens = tokenizeShellLead(command)
        var index = 0
        while index < tokens.count && isEnvAssignmentToken(tokens[index]) {
            index += 1
        }
        guard index < tokens.count else { return .empty }
        let rawExecutable = tokens[index]
        guard !rawExecutable.isEmpty else { return .empty }

        if rawExecutable.hasPrefix("~") {
            let expanded = expandTilde(rawExecutable, homePath: userHome.path)
            return fileExistsExecutable(expanded)
                ? .found(executable: rawExecutable, resolvedPath: expanded)
                : .notFound(executable: rawExecutable)
        }
        if rawExecutable.contains("/") {
            return fileExistsExecutable(rawExecutable)
                ? .found(executable: rawExecutable, resolvedPath: rawExecutable)
                : .notFound(executable: rawExecutable)
        }
        if shellBuiltins.contains(rawExecutable) {
            return .shellBuiltin(rawExecutable)
        }
        for directory in searchPath.split(separator: ":").map(String.init) where !directory.isEmpty {
            let candidate = directory.hasSuffix("/")
                ? directory + rawExecutable
                : directory + "/" + rawExecutable
            if fileExistsExecutable(candidate) {
                return .found(executable: rawExecutable, resolvedPath: candidate)
            }
        }
        return .notFound(executable: rawExecutable)
    }

    private static func expandTilde(_ value: String, homePath: String) -> String {
        if value == "~" { return homePath }
        if value.hasPrefix("~/") {
            return homePath + String(value.dropFirst())
        }
        return value
    }

    private static func isEnvAssignmentToken(_ token: String) -> Bool {
        guard let eq = token.firstIndex(of: "="), eq != token.startIndex else { return false }
        let name = token[..<eq]
        guard let first = name.first, first.isLetter || first == "_" else { return false }
        return name.allSatisfy { $0.isLetter || $0.isNumber || $0 == "_" }
    }

    private static func tokenizeShellLead(_ command: String) -> [String] {
        var tokens: [String] = []
        var current = ""
        var quote: Character?
        var escaping = false

        for ch in command {
            if escaping {
                current.append(ch)
                escaping = false
                continue
            }
            if let activeQuote = quote {
                if activeQuote == "\"" && ch == "\\" {
                    escaping = true
                } else if ch == activeQuote {
                    quote = nil
                } else {
                    current.append(ch)
                }
                continue
            }
            if ch == "\\" {
                escaping = true
            } else if ch == "\"" || ch == "'" {
                quote = ch
            } else if ch.isWhitespace {
                if !current.isEmpty {
                    tokens.append(current)
                    current = ""
                }
            } else if ch == ";" || ch == "|" || ch == "&" {
                break
            } else {
                current.append(ch)
            }
        }
        if !current.isEmpty {
            tokens.append(current)
        }
        return tokens
    }
}

enum ProcessTreeTerminator {
    static func descendantPIDs(of rootPID: Int32) -> [Int32] {
        let snapshot = Process()
        snapshot.executableURL = URL(fileURLWithPath: "/bin/ps")
        snapshot.arguments = ["-axo", "pid=,ppid="]
        let pipe = Pipe()
        snapshot.standardOutput = pipe
        snapshot.standardError = FileHandle.nullDevice
        guard (try? snapshot.run()) != nil else { return [] }
        let output = pipe.fileHandleForReading.readDataToEndOfFile()
        snapshot.waitUntilExit()
        let text = String(decoding: output, as: UTF8.self)
        var children: [Int32: [Int32]] = [:]
        for line in text.split(separator: "\n") {
            let fields = line.split(whereSeparator: \.isWhitespace)
            guard fields.count == 2,
                  let pid = Int32(fields[0]),
                  let parent = Int32(fields[1]) else { continue }
            children[parent, default: []].append(pid)
        }
        var ordered: [Int32] = []
        var visited: Set<Int32> = [rootPID]
        func appendDescendants(of pid: Int32) {
            for child in children[pid, default: []] where visited.insert(child).inserted {
                appendDescendants(of: child)
                ordered.append(child)
            }
        }
        appendDescendants(of: rootPID)
        return ordered
    }

    static func signalTree(rootPID: Int32, signal: Int32) -> [Int32] {
        let descendants = descendantPIDs(of: rootPID)
        for pid in descendants {
            _ = kill(pid, signal)
        }
        _ = kill(rootPID, signal)
        return descendants
    }

    static func isProcessAlive(_ pid: Int32) -> Bool {
        if kill(pid, 0) == 0 { return true }
        return errno == EPERM
    }

    static func terminateAndWait(process: Process, graceSeconds: TimeInterval = 0.5) async {
        let rootPID = process.processIdentifier
        guard rootPID > 0 else { return }
        var trackedPIDs = Set(signalTree(rootPID: rootPID, signal: SIGTERM))
        if process.isRunning {
            process.terminate()
        }

        let deadline = Date().addingTimeInterval(max(0.05, graceSeconds))
        while Date() < deadline {
            let anyAlive = process.isRunning || trackedPIDs.contains(where: isProcessAlive)
            if !anyAlive { break }
            try? await Task.sleep(for: .milliseconds(25))
        }

        let freshDescendants = signalTree(rootPID: rootPID, signal: SIGKILL)
        trackedPIDs.formUnion(freshDescendants)
        for pid in trackedPIDs where isProcessAlive(pid) {
            _ = kill(pid, SIGKILL)
        }
        if process.isRunning {
            _ = kill(rootPID, SIGKILL)
        }

        let killDeadline = Date().addingTimeInterval(0.5)
        while Date() < killDeadline {
            let anyAlive = process.isRunning || trackedPIDs.contains(where: isProcessAlive)
            if !anyAlive { break }
            try? await Task.sleep(for: .milliseconds(20))
        }
    }
}

@MainActor
final class AuthHelperTestRunner: ObservableObject {
    enum Status: Equatable {
        case idle
        case running(lines: [String], detectedURLs: [URL])
        case succeeded(credentialLength: Int, durationSeconds: Double, lines: [String])
        case failed(message: String, lines: [String])
    }

    private final class CancellationFlag: @unchecked Sendable {
        private let lock = NSLock()
        private var cancelled = false

        func markCancelled() {
            lock.lock()
            cancelled = true
            lock.unlock()
        }

        var isCancelled: Bool {
            lock.lock()
            defer { lock.unlock() }
            return cancelled
        }
    }

    @Published private(set) var status: Status = .idle

    private var activeProcess: Process?
    private var activePipes: (stdout: Pipe, stderr: Pipe)?
    private var activeCancellation: CancellationFlag?
    private var pendingTeardown: Task<Void, Never>?
    private var runToken = UUID()
    private var liveDisplayLines: [String] = []
    private var detectedURLs: [URL] = []
    private var heldStdoutLine: String?

    enum OutputStream {
        case stdout
        case stderr
    }

    func reset() {
        cancel()
        liveDisplayLines = []
        detectedURLs = []
        heldStdoutLine = nil
        status = .idle
    }

    func cancel() {
        runToken = UUID()
        activeCancellation?.markCancelled()
        activeCancellation = nil
        if let pipes = activePipes {
            pipes.stdout.fileHandleForReading.readabilityHandler = nil
            pipes.stderr.fileHandleForReading.readabilityHandler = nil
            try? pipes.stdout.fileHandleForReading.close()
            try? pipes.stderr.fileHandleForReading.close()
            activePipes = nil
        }
        if let process = activeProcess {
            activeProcess = nil
            let priorTeardown = pendingTeardown
            pendingTeardown = Task.detached(priority: .userInitiated) {
                await priorTeardown?.value
                await ProcessTreeTerminator.terminateAndWait(process: process)
            }
        }
        liveDisplayLines = []
        detectedURLs = []
        heldStdoutLine = nil
        if case .running = status {
            status = .idle
        }
    }

    func waitForTeardown() async {
        await pendingTeardown?.value
    }

    func run(command: String) {
        let trimmed = command.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else {
            status = .failed(message: "Enter an api_key_helper command before testing.", lines: [])
            return
        }

        cancel()
        let token = UUID()
        runToken = token
        let priorTeardown = pendingTeardown
        status = .running(lines: ["Resolving login-shell PATH…"], detectedURLs: [])

        let inherited = ProcessInfo.processInfo.environment
        let userHome = FileManager.default.homeDirectoryForCurrentUser

        Task.detached(priority: .userInitiated) { [weak self] in
            await priorTeardown?.value
            let loginPath = LoginShellPathResolver.captureLoginShellPath(inherited: inherited, userHome: userHome)
            let searchPath = LoginShellPathResolver.mergedSearchPath(
                loginShellPath: loginPath,
                inheritedPath: inherited["PATH"],
                userHome: userHome
            )
            let resolution = LoginShellPathResolver.resolveCommandExecutable(
                trimmed,
                searchPath: searchPath,
                userHome: userHome
            )

            await MainActor.run {
                guard let self, self.runToken == token else { return }
                switch resolution {
                case .empty:
                    self.status = .failed(message: "Enter an api_key_helper command before testing.", lines: [])
                    return
                case .notFound(let executable):
                    self.status = .failed(
                        message: "Command '\(executable)' was not found on your shell PATH. Make sure it is installed and available in a login shell.",
                        lines: ["Searched PATH: \(searchPath)"]
                    )
                    return
                case .shellBuiltin, .found:
                    self.launchHelperProcess(
                        command: trimmed,
                        searchPath: searchPath,
                        inherited: inherited,
                        userHome: userHome,
                        token: token
                    )
                }
            }
        }
    }

    private func launchHelperProcess(
        command: String,
        searchPath: String,
        inherited: [String: String],
        userHome: URL,
        token: UUID
    ) {
        let start = Date()
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", command]
        process.currentDirectoryURL = userHome
        process.standardInput = FileHandle.nullDevice

        var environment: [String: String] = [
            "PATH": searchPath,
            "HOME": userHome.path,
            "TMPDIR": inherited["TMPDIR"] ?? NSTemporaryDirectory(),
            "LANG": inherited["LANG"] ?? "en_US.UTF-8",
        ]
        for key in SidecarLaunchEnvironment.safeInheritedKeys {
            if let value = inherited[key], !value.isEmpty {
                environment[key] = value
            }
        }
        process.environment = environment

        let stdoutPipe = Pipe()
        let stderrPipe = Pipe()
        process.standardOutput = stdoutPipe
        process.standardError = stderrPipe

        final class StreamCollector: @unchecked Sendable {
            private let lock = NSLock()
            private var stdoutBuffer = Data()
            private var stderrBuffer = Data()
            private var stdoutLines: [String] = []
            private var stderrLines: [String] = []

            func appendStdout(_ data: Data) -> [String] {
                lock.lock()
                defer { lock.unlock() }
                stdoutBuffer.append(data)
                return extractLines(from: &stdoutBuffer, into: &stdoutLines)
            }

            func appendStderr(_ data: Data) -> [String] {
                lock.lock()
                defer { lock.unlock() }
                stderrBuffer.append(data)
                return extractLines(from: &stderrBuffer, into: &stderrLines)
            }

            func finalize() -> (stdout: [String], stderr: [String]) {
                lock.lock()
                defer { lock.unlock() }
                if !stdoutBuffer.isEmpty {
                    let rem = String(decoding: stdoutBuffer, as: UTF8.self)
                        .trimmingCharacters(in: CharacterSet.newlines)
                    if !rem.isEmpty { stdoutLines.append(rem) }
                    stdoutBuffer.removeAll()
                }
                if !stderrBuffer.isEmpty {
                    let rem = String(decoding: stderrBuffer, as: UTF8.self)
                        .trimmingCharacters(in: CharacterSet.newlines)
                    if !rem.isEmpty { stderrLines.append(rem) }
                    stderrBuffer.removeAll()
                }
                return (stdoutLines, stderrLines)
            }

            private func extractLines(from buffer: inout Data, into target: inout [String]) -> [String] {
                var emitted: [String] = []
                while let nl = buffer.firstIndex(of: 0x0A) {
                    var lineData = Data(buffer[..<nl])
                    buffer.removeSubrange(...nl)
                    if lineData.last == 0x0D { lineData.removeLast() }
                    let line = String(decoding: lineData, as: UTF8.self)
                    target.append(line)
                    emitted.append(line)
                }
                return emitted
            }
        }

        let collector = StreamCollector()
        let cancellation = CancellationFlag()
        activeCancellation = cancellation
        activePipes = (stdout: stdoutPipe, stderr: stderrPipe)
        liveDisplayLines = []
        detectedURLs = []
        heldStdoutLine = nil
        status = .running(lines: ["Running auth helper…"], detectedURLs: [])

        stdoutPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            guard !cancellation.isCancelled else {
                handle.readabilityHandler = nil
                return
            }
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                return
            }
            let emitted = collector.appendStdout(data)
            guard !emitted.isEmpty else { return }
            Task { @MainActor in
                guard let self, self.runToken == token else { return }
                self.ingestLiveLines(emitted, stream: .stdout)
            }
        }

        stderrPipe.fileHandleForReading.readabilityHandler = { [weak self] handle in
            guard !cancellation.isCancelled else {
                handle.readabilityHandler = nil
                return
            }
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                return
            }
            let emitted = collector.appendStderr(data)
            guard !emitted.isEmpty else { return }
            Task { @MainActor in
                guard let self, self.runToken == token else { return }
                self.ingestLiveLines(emitted, stream: .stderr)
            }
        }

        process.terminationHandler = { [weak self] terminated in
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            if cancellation.isCancelled {
                return
            }
            let remainingOut = stdoutPipe.fileHandleForReading.readDataToEndOfFile()
            if !remainingOut.isEmpty { _ = collector.appendStdout(remainingOut) }
            let remainingErr = stderrPipe.fileHandleForReading.readDataToEndOfFile()
            if !remainingErr.isEmpty { _ = collector.appendStderr(remainingErr) }
            let (stdoutLines, stderrLines) = collector.finalize()
            let duration = Date().timeIntervalSince(start)
            let exitCode = terminated.terminationStatus

            Task { @MainActor in
                guard let self, self.runToken == token else { return }
                self.activeProcess = nil
                self.activePipes = nil
                self.activeCancellation = nil
                let nonEmptyOut = stdoutLines.map { $0.trimmingCharacters(in: .whitespacesAndNewlines) }.filter { !$0.isEmpty }
                let nonEmptyErr = stderrLines.map { SidecarLogRecorder.defaultRedact($0.trimmingCharacters(in: .whitespacesAndNewlines)) }.filter { !$0.isEmpty }

                guard exitCode == 0 else {
                    let detail = nonEmptyErr.last ?? "Helper exited with status \(exitCode)."
                    self.status = .failed(
                        message: "Auth helper failed (exit \(exitCode)): \(detail)",
                        lines: Array(nonEmptyErr.suffix(8))
                    )
                    return
                }

                guard let credential = nonEmptyOut.last, !credential.isEmpty else {
                    self.status = .failed(
                        message: "Auth helper exited 0 but emitted no credential on stdout.",
                        lines: Array(nonEmptyErr.suffix(8))
                    )
                    return
                }

                // Per REQ-CREDHELPER-001, lines before the final credential line are instructions;
                // never display the final credential line itself.
                let instructionLines = nonEmptyOut.dropLast().map { SidecarLogRecorder.defaultRedact($0) }
                self.status = .succeeded(
                    credentialLength: credential.count,
                    durationSeconds: duration,
                    lines: Array(instructionLines.suffix(6))
                )
            }
        }

        do {
            try process.run()
            activeProcess = process
        } catch {
            stdoutPipe.fileHandleForReading.readabilityHandler = nil
            stderrPipe.fileHandleForReading.readabilityHandler = nil
            activePipes = nil
            activeCancellation = nil
            status = .failed(message: "Could not start auth helper: \(error.localizedDescription)", lines: [])
        }
    }

    private func ingestLiveLines(_ newLines: [String], stream: OutputStream) {
        for rawLine in newLines {
            let trimmed = rawLine.trimmingCharacters(in: .whitespacesAndNewlines)
            guard !trimmed.isEmpty else { continue }
            switch stream {
            case .stderr:
                // stderr never carries the credential (REQ-CREDHELPER-001); preserve every
                // instruction line including standalone device codes like ABCD-EFGH.
                recordDisplayLine(trimmed)
            case .stdout:
                // The last non-empty stdout line is the credential (REQ-CREDHELPER-001).
                // Hold back only the most recent stdout line; once another line arrives, the
                // previous one is guaranteed not to be the credential. If the held line itself
                // contains an interactive URL (e.g. OIDC sign-in prompt while waiting), show it
                // immediately so the user can complete browser auth.
                if let previous = heldStdoutLine {
                    recordDisplayLine(previous)
                    heldStdoutLine = nil
                }
                let urls = Self.extractHTTPSURLs(from: trimmed)
                if !urls.isEmpty {
                    recordDisplayLine(trimmed)
                } else {
                    heldStdoutLine = trimmed
                }
            }
        }
        if case .running = status {
            status = .running(
                lines: liveDisplayLines.isEmpty ? ["Running auth helper…"] : liveDisplayLines,
                detectedURLs: detectedURLs
            )
        }
    }

    private func recordDisplayLine(_ line: String) {
        for url in Self.extractHTTPSURLs(from: line) where !detectedURLs.contains(url) {
            detectedURLs.append(url)
        }
        liveDisplayLines.append(SidecarLogRecorder.defaultRedact(line))
        if liveDisplayLines.count > 20 {
            liveDisplayLines = Array(liveDisplayLines.suffix(20))
        }
    }

    static func extractHTTPSURLs(from text: String) -> [URL] {
        guard let regex = try? NSRegularExpression(pattern: #"https?://[^\s)>\]]+"#) else {
            return []
        }
        let range = NSRange(text.startIndex..<text.endIndex, in: text)
        return regex.matches(in: text, range: range).compactMap { match in
            guard let swiftRange = Range(match.range, in: text) else { return nil }
            return URL(string: String(text[swiftRange]))
        }
    }
}

struct LaunchdPlistHint: Equatable, Sendable {
    let port: Int
    let tlsEnabled: Bool
}

struct DetectedLocalServer: Equatable, Sendable {
    let origin: PhoenixOrigin
    let version: VersionInfo?
    let tlsTrustRequired: Bool
    let isLaunchdInstalled: Bool
}

enum LocalDeploymentDetector {
    static let defaultLaunchdLabel = "com.phoenix-ide.server"
    static let defaultPort = 8031

    static func launchdPlistURL(userHome: URL = FileManager.default.homeDirectoryForCurrentUser) -> URL {
        userHome
            .appendingPathComponent("Library", isDirectory: true)
            .appendingPathComponent("LaunchAgents", isDirectory: true)
            .appendingPathComponent("\(defaultLaunchdLabel).plist")
    }

    static func parseLaunchdPlist(data: Data) -> LaunchdPlistHint? {
        guard let plist = try? PropertyListSerialization.propertyList(from: data, format: nil) as? [String: Any] else {
            return nil
        }
        let env = plist["EnvironmentVariables"] as? [String: Any] ?? [:]

        var port = defaultPort
        if let sockets = plist["Sockets"] as? [String: Any],
           let listeners = sockets["Listeners"] as? [String: Any],
           let rawService = listeners["SockServiceName"],
           let parsedPort = intValue(rawService),
           (1...65_535).contains(parsedPort) {
            port = parsedPort
        } else if let rawEnvPort = env["PHOENIX_PORT"],
                  let parsedPort = intValue(rawEnvPort),
                  (1...65_535).contains(parsedPort) {
            port = parsedPort
        }

        let tlsMode = (env["PHOENIX_TLS"] as? String)?
            .trimmingCharacters(in: .whitespacesAndNewlines)
            .lowercased() ?? ""
        let certPath = (env["PHOENIX_TLS_CERT_PATH"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let keyPath = (env["PHOENIX_TLS_KEY_PATH"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let hasManualPaths = !certPath.isEmpty && !keyPath.isEmpty
        let tlsEnabled = hasManualPaths || ["1", "true", "on", "auto", "manual"].contains(tlsMode)

        return LaunchdPlistHint(port: port, tlsEnabled: tlsEnabled)
    }

    static func candidateOrigins(hint: LaunchdPlistHint?) -> [PhoenixOrigin] {
        let port = hint?.port ?? defaultPort
        let preferHTTPS = hint?.tlsEnabled ?? true
        let schemes = preferHTTPS ? ["https", "http"] : ["http", "https"]
        return schemes.compactMap { try? PhoenixOrigin("\($0)://localhost:\(port)") }
    }

    static func detect(
        userHome: URL = FileManager.default.homeDirectoryForCurrentUser,
        timeoutSeconds: TimeInterval = 1.5
    ) async -> DetectedLocalServer? {
        let plistURL = launchdPlistURL(userHome: userHome)
        let hint: LaunchdPlistHint? = (try? Data(contentsOf: plistURL)).flatMap(parseLaunchdPlist)
        let isLaunchdInstalled = hint != nil
        let candidates = candidateOrigins(hint: hint)

        var tlsBlockedOrigin: PhoenixOrigin?
        for origin in candidates {
            let outcome = await probeVersion(at: origin, timeoutSeconds: timeoutSeconds)
            switch outcome {
            case .found(let version):
                return DetectedLocalServer(
                    origin: origin,
                    version: version,
                    tlsTrustRequired: false,
                    isLaunchdInstalled: isLaunchdInstalled
                )
            case .tlsTrustFailure:
                tlsBlockedOrigin = origin
            case .unavailable:
                continue
            }
        }

        if let tlsBlockedOrigin {
            return DetectedLocalServer(
                origin: tlsBlockedOrigin,
                version: nil,
                tlsTrustRequired: true,
                isLaunchdInstalled: isLaunchdInstalled
            )
        }
        return nil
    }

    enum ProbeOutcome: Equatable, Sendable {
        case found(VersionInfo)
        case tlsTrustFailure
        case unavailable
    }

    static func probeVersion(at origin: PhoenixOrigin, timeoutSeconds: TimeInterval = 1.5) async -> ProbeOutcome {
        var request = URLRequest(url: origin.url(path: "/api/version"))
        request.timeoutInterval = timeoutSeconds
        let delegate = RedirectRejectingURLSessionDelegate(expectedOrigin: origin)
        let session = URLSession(configuration: .ephemeral, delegate: delegate, delegateQueue: nil)
        defer { session.invalidateAndCancel() }
        do {
            let (data, response) = try await session.data(for: request)
            guard let http = response as? HTTPURLResponse,
                  http.statusCode == 200,
                  let finalURL = response.url,
                  origin.exactlyMatches(finalURL),
                  let version = try? JSONDecoder().decode(VersionInfo.self, from: data) else {
                return .unavailable
            }
            return .found(version)
        } catch let urlError as URLError {
            if urlError.code == .serverCertificateUntrusted
                || urlError.code == .serverCertificateHasUnknownRoot
                || urlError.code == .serverCertificateHasBadDate
                || urlError.code == .serverCertificateNotYetValid {
                return .tlsTrustFailure
            }
            return .unavailable
        } catch {
            return .unavailable
        }
    }

    private static func intValue(_ value: Any) -> Int? {
        if let intVal = value as? Int { return intVal }
        if let strVal = value as? String { return Int(strVal.trimmingCharacters(in: .whitespacesAndNewlines)) }
        return nil
    }
}
