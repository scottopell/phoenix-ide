import Foundation
import Security

// Only non-secret preferences are stored in UserDefaults.
enum PreferenceKey {
    static let serverMode = "serverMode"
    static let attachedOrigin = "attachedOrigin"
    static let bundledPort = "bundledPort"
    static let bundledDevelopmentBinary = "bundledDevelopmentBinary"
    static let rustLogLevel = "rustLogLevel"
    static let legacyAnthropicAPIKey = "anthropicApiKey"
}

enum ServerModeKind: String, CaseIterable, Identifiable {
    case attached
    case bundled

    var id: String { rawValue }

    var label: String {
        switch self {
        case .attached: "Managed deployment"
        case .bundled: "Bundled Phoenix"
        }
    }
}

struct PhoenixOrigin: Equatable, Codable, CustomStringConvertible {
    let url: URL

    init(_ rawValue: String) throws {
        let trimmed = rawValue.trimmingCharacters(in: .whitespacesAndNewlines)
        guard var components = URLComponents(string: trimmed),
              let scheme = components.scheme?.lowercased(),
              scheme == "http" || scheme == "https",
              components.host?.isEmpty == false,
              components.user == nil,
              components.password == nil,
              components.query == nil,
              components.fragment == nil,
              components.path.isEmpty || components.path == "/" else {
            throw ConfigurationError.invalidOrigin
        }
        components.scheme = scheme
        components.path = ""
        guard let normalized = components.url else { throw ConfigurationError.invalidOrigin }
        url = normalized
    }

    var description: String { url.absoluteString }

    func url(path: String) -> URL {
        let relative = path.hasPrefix("/") ? String(path.dropFirst()) : path
        return URL(string: relative, relativeTo: url.appendingPathComponent("/"))!.absoluteURL
    }

    func exactlyMatches(_ candidate: URL) -> Bool {
        guard let lhs = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let rhs = URLComponents(url: candidate, resolvingAgainstBaseURL: false) else { return false }
        return lhs.scheme?.lowercased() == rhs.scheme?.lowercased()
            && lhs.host?.lowercased() == rhs.host?.lowercased()
            && effectivePort(lhs) == effectivePort(rhs)
    }

    private func effectivePort(_ value: URLComponents) -> Int? {
        value.port ?? (value.scheme?.lowercased() == "https" ? 443 : 80)
    }
}

enum ConfigurationError: LocalizedError {
    case invalidOrigin
    case invalidPort
    case bundledBinaryMissing
    case bundledDataInUse

    var errorDescription: String? {
        switch self {
        case .invalidOrigin: "Enter an HTTP or HTTPS origin without a path, credentials, query, or fragment."
        case .invalidPort: "Bundled Phoenix port must be between 1024 and 65535."
        case .bundledBinaryMissing: "The Phoenix sidecar is not present in this app bundle."
        case .bundledDataInUse: "Another Phoenix.app instance owns the bundled data directory."
        }
    }
}

struct AttachedServerConfiguration: Equatable {
    let origin: PhoenixOrigin
}

struct BundledServerConfiguration: Equatable {
    let origin: PhoenixOrigin
    let executableURL: URL
    let databaseURL: URL
    let logURL: URL
    let ownerLockURL: URL
    let rustLogLevel: String

    var publicEnvironment: [String: String] {
        [
            "PHOENIX_BIND_ADDR": "127.0.0.1",
            "PHOENIX_TLS": "off",
            "PHOENIX_PORT": String(origin.url.port!),
            "PHOENIX_DB_PATH": databaseURL.path,
            "RUST_LOG": rustLogLevel,
        ]
    }
}

enum ServerMode: Equatable {
    case attached(AttachedServerConfiguration)
    case bundled(BundledServerConfiguration)

    var origin: PhoenixOrigin {
        switch self {
        case .attached(let configuration): configuration.origin
        case .bundled(let configuration): configuration.origin
        }
    }

    var kind: ServerModeKind {
        switch self {
        case .attached: .attached
        case .bundled: .bundled
        }
    }
}

enum ConfigurationStore {
    static let defaultAttachedOrigin = "https://localhost:8031"
    static let defaultBundledPort = 8420

    static func load(bundle: Bundle = .main, defaults: UserDefaults = .standard) throws -> ServerMode {
        let kind = defaults.string(forKey: PreferenceKey.serverMode)
            .flatMap(ServerModeKind.init(rawValue:)) ?? .attached
        switch kind {
        case .attached:
            let raw = defaults.string(forKey: PreferenceKey.attachedOrigin) ?? defaultAttachedOrigin
            return .attached(AttachedServerConfiguration(origin: try PhoenixOrigin(raw)))
        case .bundled:
            let storedPort = defaults.integer(forKey: PreferenceKey.bundledPort)
            let port = storedPort == 0 ? defaultBundledPort : storedPort
            guard (1024...65535).contains(port) else { throw ConfigurationError.invalidPort }
            let root = try applicationSupportRoot()
            let executable = bundledExecutable(bundle: bundle, defaults: defaults)
            guard FileManager.default.isExecutableFile(atPath: executable.path) else {
                throw ConfigurationError.bundledBinaryMissing
            }
            return .bundled(BundledServerConfiguration(
                origin: try PhoenixOrigin("http://127.0.0.1:\(port)"),
                executableURL: executable,
                databaseURL: root.appendingPathComponent("phoenix.db"),
                logURL: root.appendingPathComponent("phoenix.log"),
                ownerLockURL: root.appendingPathComponent("owner.lock"),
                rustLogLevel: defaults.string(forKey: PreferenceKey.rustLogLevel) ?? "phoenix_ide=info"
            ))
        }
    }

    static func applicationSupportRoot() throws -> URL {
        let base = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let root = base.appendingPathComponent("Phoenix", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        return root
    }

    private static func bundledExecutable(bundle: Bundle, defaults: UserDefaults) -> URL {
        #if DEBUG
        if let override = defaults.string(forKey: PreferenceKey.bundledDevelopmentBinary), !override.isEmpty {
            return URL(fileURLWithPath: NSString(string: override).expandingTildeInPath)
        }
        #endif
        return bundle.bundleURL.appendingPathComponent("Contents/Helpers/phoenix_ide")
    }

    static func removeLegacyPlaintextSecret(defaults: UserDefaults = .standard) {
        // The old app wrote this directly to its preferences plist. It is deliberately
        // deleted rather than silently migrating a secret into a differently-owned app.
        defaults.removeObject(forKey: PreferenceKey.legacyAnthropicAPIKey)
    }
}

enum ProviderSecret: String, CaseIterable {
    case anthropicAPIKey = "anthropic-api-key"
    case openAIAPIKey = "openai-api-key"

    var environmentKey: String {
        switch self {
        case .anthropicAPIKey: "ANTHROPIC_API_KEY"
        case .openAIAPIKey: "OPENAI_API_KEY"
        }
    }

    var label: String {
        switch self {
        case .anthropicAPIKey: "Anthropic API key"
        case .openAIAPIKey: "OpenAI API key"
        }
    }
}

struct KeychainStore {
    let service: String

    init(service: String = "com.phoenixide.macos.sidecar") {
        self.service = service
    }

    func read(_ secret: ProviderSecret) throws -> String? {
        var query = baseQuery(secret)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound { return nil }
        guard status == errSecSuccess,
              let data = result as? Data,
              let value = String(data: data, encoding: .utf8) else {
            throw KeychainError.status(status)
        }
        return value
    }

    func write(_ value: String, for secret: ProviderSecret) throws {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.isEmpty {
            let status = SecItemDelete(baseQuery(secret) as CFDictionary)
            guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError.status(status) }
            return
        }
        let data = Data(trimmed.utf8)
        let updateStatus = SecItemUpdate(
            baseQuery(secret) as CFDictionary,
            [kSecValueData as String: data] as CFDictionary
        )
        if updateStatus == errSecSuccess { return }
        guard updateStatus == errSecItemNotFound else { throw KeychainError.status(updateStatus) }
        var item = baseQuery(secret)
        item[kSecValueData as String] = data
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        let addStatus = SecItemAdd(item as CFDictionary, nil)
        guard addStatus == errSecSuccess else { throw KeychainError.status(addStatus) }
    }

    func processEnvironment() -> [String: String] {
        var result: [String: String] = [:]
        for secret in ProviderSecret.allCases {
            if let value = try? read(secret), !value.isEmpty {
                result[secret.environmentKey] = value
            }
        }
        return result
    }

    private func baseQuery(_ secret: ProviderSecret) -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: secret.rawValue,
        ]
    }
}

enum KeychainError: LocalizedError {
    case status(OSStatus)

    var errorDescription: String? {
        switch self {
        case .status(let status): SecCopyErrorMessageString(status, nil) as String? ?? "Keychain error \(status)"
        }
    }
}

enum PhoenixURLAction: Equatable {
    case open
    case status
    case new(prompt: String, cwd: String?)

    init?(url: URL) {
        guard url.scheme?.lowercased() == "phoenix" else { return nil }
        let rawAction = url.host?.isEmpty == false
            ? url.host!
            : url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
        switch rawAction.lowercased() {
        case "", "open": self = .open
        case "status", "debug": self = .status
        case "new":
            let components = URLComponents(url: url, resolvingAgainstBaseURL: false)
            let prompt = components?.queryItems?.first(where: { $0.name == "prompt" })?.value ?? ""
            let cwd = components?.queryItems?.first(where: { $0.name == "cwd" })?.value
            self = .new(prompt: prompt, cwd: cwd)
        default: self = .open
        }
    }
}

struct ConversationCreationPayload: Equatable {
    let cwd: String
    let model: String
    let messageID: String

    var dictionary: [String: Any] {
        [
            "cwd": cwd,
            "model": model,
            "text": "",
            "message_id": messageID,
            "images": [],
            "files": [],
            "mode": "direct",
            "seed_label": "External prompt",
        ]
    }
}

struct VersionInfo: Codable, Equatable {
    let version: String
    let gitSHA: String

    enum CodingKeys: String, CodingKey {
        case version
        case gitSHA = "git_sha"
    }
}

struct DeploymentInfo: Codable, Equatable {
    let build: BuildInfo
    let network: NetworkInfo
    let localAccess: Bool
    let installationOwnership: InstallationOwnership

    enum CodingKeys: String, CodingKey {
        case build, network
        case localAccess = "local_access"
        case installationOwnership = "installation_ownership"
    }
}

struct BuildInfo: Codable, Equatable {
    let version: String
    let gitSHA: String

    enum CodingKeys: String, CodingKey {
        case version
        case gitSHA = "git_sha"
    }
}

struct NetworkInfo: Codable, Equatable {
    let bindAddress: String
    let socketActivated: Bool
    let tls: TLSInfo

    enum CodingKeys: String, CodingKey {
        case bindAddress = "bind_address"
        case socketActivated = "socket_activated"
        case tls
    }
}

struct TLSInfo: Codable, Equatable {
    let enabled: Bool
    let mode: String?
}

enum InstallationOwnership: Equatable, Codable {
    case launchdManaged
    case systemdManaged
    case bareSupervisorManaged
    case development
    case unmanaged(String)
    case ambiguous(String)
    case unsupported(String)
    case unknown(String)

    var label: String {
        switch self {
        case .launchdManaged: "launchd managed"
        case .systemdManaged: "systemd managed"
        case .bareSupervisorManaged: "bare supervisor managed"
        case .development: "development"
        case .unmanaged(let reason): "unmanaged: \(reason)"
        case .ambiguous(let reason): "ambiguous: \(reason)"
        case .unsupported(let platform): "unsupported: \(platform)"
        case .unknown(let kind): "unknown: \(kind)"
        }
    }

    var grantsManagedAuthority: Bool {
        if case .launchdManaged = self { return true }
        return false
    }

    private enum CodingKeys: String, CodingKey { case kind, reason, platform }

    init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        let kind = try container.decode(String.self, forKey: .kind)
        switch kind {
        case "launchd_managed": self = .launchdManaged
        case "systemd_managed": self = .systemdManaged
        case "bare_supervisor_managed": self = .bareSupervisorManaged
        case "development": self = .development
        case "unmanaged": self = .unmanaged(try container.decode(String.self, forKey: .reason))
        case "ambiguous": self = .ambiguous(try container.decode(String.self, forKey: .reason))
        case "unsupported": self = .unsupported(try container.decode(String.self, forKey: .platform))
        default: self = .unknown(kind)
        }
    }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        switch self {
        case .launchdManaged: try container.encode("launchd_managed", forKey: .kind)
        case .systemdManaged: try container.encode("systemd_managed", forKey: .kind)
        case .bareSupervisorManaged: try container.encode("bare_supervisor_managed", forKey: .kind)
        case .development: try container.encode("development", forKey: .kind)
        case .unmanaged(let reason):
            try container.encode("unmanaged", forKey: .kind); try container.encode(reason, forKey: .reason)
        case .ambiguous(let reason):
            try container.encode("ambiguous", forKey: .kind); try container.encode(reason, forKey: .reason)
        case .unsupported(let platform):
            try container.encode("unsupported", forKey: .kind); try container.encode(platform, forKey: .platform)
        case .unknown(let kind): try container.encode(kind, forKey: .kind)
        }
    }
}
