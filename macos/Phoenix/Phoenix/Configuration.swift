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

enum PendingServerModeKind: String, CaseIterable, Identifiable {
    case attached
    case bundled

    var id: String { rawValue }

    var label: String {
        switch self {
        case .attached: "Managed deployment"
        case .bundled: "Bundled Phoenix"
        }
    }

    var persistedKind: ServerModeKind {
        switch self {
        case .attached: .attached
        case .bundled: .bundled
        }
    }

    init(_ persistedKind: ServerModeKind) {
        switch persistedKind {
        case .attached: self = .attached
        case .bundled: self = .bundled
        }
    }
}

struct SettingsDraft: Equatable {
    var mode: PendingServerModeKind?
    var attachedOrigin: String
    var bundledPort: Int
    var developmentBinaryOverride: String
    var rustLogLevel: String
    var anthropicKey: String
    var openAIKey: String

    static let defaults = SettingsDraft(
        mode: nil,
        attachedOrigin: ConfigurationStore.defaultAttachedOrigin,
        bundledPort: ConfigurationStore.defaultBundledPort,
        developmentBinaryOverride: "",
        rustLogLevel: "phoenix_ide=info",
        anthropicKey: "",
        openAIKey: ""
    )
}

struct DraftLoadResult: Equatable {
    let draft: SettingsDraft
    let hasSavedModeSelection: Bool
}

struct SettingsPersistenceSummary: Equatable {
    let requiresReconnect: Bool
    let savedSecrets: [ProviderSecret]
    let deletedSecrets: [ProviderSecret]
}

struct ReopenDecision {
    static func shouldShowMainWindow(mainWindowIsVisible: Bool) -> Bool {
        !mainWindowIsVisible
    }
}

struct SidecarPackagingValidation {
    enum SigningExpectation: Equatable {
        case none
        case adHoc
        case identity(String)
    }

    struct Result: Equatable {
        let helperExists: Bool
        let missingArchitectures: [String]
        let signingMismatch: Bool
    }

    static func validatePackagedSidecar(
        helperExists: Bool,
        actualArchitectures: [String],
        requiredArchitectures: [String],
        actualSigningIdentity: String?,
        expectedSigning: SigningExpectation
    ) -> Result {
        let missingArchitectures = requiredArchitectures.filter { !actualArchitectures.contains($0) }
        let signingMismatch: Bool = switch expectedSigning {
        case .none:
            false
        case .adHoc:
            actualSigningIdentity != "-"
        case .identity(let identity):
            actualSigningIdentity != identity
        }
        return Result(
            helperExists: helperExists,
            missingArchitectures: missingArchitectures,
            signingMismatch: signingMismatch
        )
    }
}

protocol SecretStore {
    func read(_ secret: ProviderSecret) throws -> String?
    func write(_ value: String, for secret: ProviderSecret) throws
    func processEnvironment() throws -> [String: String]
}

struct PersistedPreferenceSnapshot: Equatable {
    let serverMode: String?
    let attachedOrigin: String?
    let bundledPort: Int?
    let developmentBinaryOverride: String?
    let rustLogLevel: String?
}

struct PersistedSettingsSnapshot: Equatable {
    let preferences: PersistedPreferenceSnapshot
    let secrets: [ProviderSecret: String?]

    func draft() -> SettingsDraft {
        let persistedMode = preferences.serverMode
            .flatMap(ServerModeKind.init(rawValue:))
            .map(PendingServerModeKind.init)
        return SettingsDraft(
            mode: persistedMode,
            attachedOrigin: preferences.attachedOrigin ?? ConfigurationStore.defaultAttachedOrigin,
            bundledPort: preferences.bundledPort ?? ConfigurationStore.defaultBundledPort,
            developmentBinaryOverride: preferences.developmentBinaryOverride ?? "",
            rustLogLevel: preferences.rustLogLevel ?? "phoenix_ide=info",
            anthropicKey: (secrets[.anthropicAPIKey] ?? nil) ?? "",
            openAIKey: (secrets[.openAIAPIKey] ?? nil) ?? ""
        )
    }
}

struct SettingsPersistence {
    let defaults: UserDefaults
    let keychain: any SecretStore
    let bundle: Bundle

    init(defaults: UserDefaults = .standard, keychain: any SecretStore = KeychainStore(), bundle: Bundle = .main) {
        self.defaults = defaults
        self.keychain = keychain
        self.bundle = bundle
    }

    func loadDraft() throws -> DraftLoadResult {
        let snapshot = try persistedSnapshot()
        let hasSavedModeSelection = snapshot.preferences.serverMode != nil
        return DraftLoadResult(draft: snapshot.draft(), hasSavedModeSelection: hasSavedModeSelection)
    }

    func persistedSnapshot() throws -> PersistedSettingsSnapshot {
        let preferences = PersistedPreferenceSnapshot(
            serverMode: defaults.string(forKey: PreferenceKey.serverMode),
            attachedOrigin: defaults.string(forKey: PreferenceKey.attachedOrigin),
            bundledPort: defaults.object(forKey: PreferenceKey.bundledPort) as? Int,
            developmentBinaryOverride: defaults.string(forKey: PreferenceKey.bundledDevelopmentBinary),
            rustLogLevel: defaults.string(forKey: PreferenceKey.rustLogLevel)
        )
        var secrets: [ProviderSecret: String?] = [:]
        for secret in ProviderSecret.allCases {
            secrets[secret] = try keychain.read(secret)
        }
        return PersistedSettingsSnapshot(preferences: preferences, secrets: secrets)
    }

    func persist(draft: SettingsDraft, appliedSnapshot previous: PersistedSettingsSnapshot? = nil) throws -> (candidate: ServerMode, summary: SettingsPersistenceSummary, persistedSnapshot: PersistedSettingsSnapshot) {
        guard let mode = draft.mode else { throw ConfigurationError.missingModeSelection }
        let candidate = try ConfigurationStore.loadCandidate(
            kind: mode.persistedKind,
            attachedOrigin: draft.attachedOrigin,
            bundledPort: draft.bundledPort,
            developmentBinaryOverride: draft.developmentBinaryOverride,
            rustLogLevel: draft.rustLogLevel,
            bundle: bundle
        )
        let priorAppliedSnapshot = try previous ?? persistedSnapshot()

        let savedSecrets = ProviderSecret.allCases.compactMap { secret -> ProviderSecret? in
            let trimmed = secretValue(in: draft, for: secret).trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty ? nil : secret
        }
        let deletedSecrets = ProviderSecret.allCases.compactMap { secret -> ProviderSecret? in
            let trimmed = secretValue(in: draft, for: secret).trimmingCharacters(in: .whitespacesAndNewlines)
            return trimmed.isEmpty && priorAppliedSnapshot.secrets[secret] != nil ? secret : nil
        }

        do {
            try apply(draft: draft)
        } catch {
            if let rollbackFailure = rollback(to: priorAppliedSnapshot) {
                throw SettingsPersistenceError.applyFailedWithRollbackFailure(cause: error, rollbackFailure: rollbackFailure)
            }
            throw error
        }

        let after = try persistedSnapshot()
        return (candidate, SettingsPersistenceSummary(
            requiresReconnect: priorAppliedSnapshot.draft() != after.draft(),
            savedSecrets: savedSecrets,
            deletedSecrets: deletedSecrets
        ), after)
    }

    private func apply(draft: SettingsDraft) throws {
        writePreferences(for: draft)
        for secret in ProviderSecret.allCases {
            try keychain.write(secretValue(in: draft, for: secret), for: secret)
        }
    }

    private func writePreferences(for draft: SettingsDraft) {
        defaults.set(draft.mode?.persistedKind.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set(draft.attachedOrigin, forKey: PreferenceKey.attachedOrigin)
        defaults.set(draft.bundledPort, forKey: PreferenceKey.bundledPort)
        defaults.set(draft.developmentBinaryOverride, forKey: PreferenceKey.bundledDevelopmentBinary)
        defaults.set(draft.rustLogLevel, forKey: PreferenceKey.rustLogLevel)
    }

    private func rollback(to snapshot: PersistedSettingsSnapshot) -> Error? {
        var rollbackFailures: [Error] = []
        restorePreferences(snapshot.preferences)
        for secret in ProviderSecret.allCases {
            do {
                try keychain.write((snapshot.secrets[secret] ?? nil) ?? "", for: secret)
            } catch {
                rollbackFailures.append(error)
            }
        }
        guard !rollbackFailures.isEmpty else { return nil }
        return RollbackFailure(errors: rollbackFailures)
    }

    private func restorePreferences(_ snapshot: PersistedPreferenceSnapshot) {
        restore(snapshot.serverMode, forKey: PreferenceKey.serverMode)
        restore(snapshot.attachedOrigin, forKey: PreferenceKey.attachedOrigin)
        restore(snapshot.bundledPort, forKey: PreferenceKey.bundledPort)
        restore(snapshot.developmentBinaryOverride, forKey: PreferenceKey.bundledDevelopmentBinary)
        restore(snapshot.rustLogLevel, forKey: PreferenceKey.rustLogLevel)
    }

    private func restore(_ value: String?, forKey key: String) {
        if let value { defaults.set(value, forKey: key) } else { defaults.removeObject(forKey: key) }
    }

    private func restore(_ value: Int?, forKey key: String) {
        if let value { defaults.set(value, forKey: key) } else { defaults.removeObject(forKey: key) }
    }

    private func secretValue(in draft: SettingsDraft, for secret: ProviderSecret) -> String {
        switch secret {
        case .anthropicAPIKey: draft.anthropicKey
        case .openAIAPIKey: draft.openAIKey
        }
    }
}

struct RollbackFailure: LocalizedError {
    let errors: [Error]

    var errorDescription: String? {
        let descriptions = errors.compactMap { $0.localizedDescription }
        guard !descriptions.isEmpty else { return "Unknown rollback failure" }
        return descriptions.joined(separator: "; ")
    }
}

enum SettingsPersistenceError: LocalizedError {
    case applyFailedWithRollbackFailure(cause: Error, rollbackFailure: Error)

    var errorDescription: String? {
        switch self {
        case .applyFailedWithRollbackFailure(let cause, let rollbackFailure):
            "Failed to save settings: \(cause.localizedDescription). Rollback also failed: \(rollbackFailure.localizedDescription)"
        }
    }
}

enum BrowserSurfaceRole: Equatable {
    case primary
    case authPopup
}

struct PhoenixWebViewPolicy {
    enum MediaCaptureDecision: Equatable {
        case grant
        case deny
    }

    enum NotificationDecision: Equatable {
        case grant
        case deny
    }

    enum PopupDecision: Equatable {
        case allowManagedChild
        case externalize
        case cancel
    }

    static func url(for securityOrigin: SecurityOriginDescriptor) -> URL? {
        let host = securityOrigin.host.contains(":") && !securityOrigin.host.hasPrefix("[")
            ? "[\(securityOrigin.host)]"
            : securityOrigin.host
        var value = "\(securityOrigin.scheme)://\(host)"
        if securityOrigin.port >= 0 {
            value += ":\(securityOrigin.port)"
        }
        return URL(string: value)
    }

    static func mediaCaptureDecision(
        for securityOrigin: SecurityOriginDescriptor,
        captureType: MediaCaptureKind,
        expectedOrigin: PhoenixOrigin
    ) -> MediaCaptureDecision {
        guard captureType == .microphone,
              let candidate = url(for: securityOrigin),
              expectedOrigin.exactlyMatches(candidate) else {
            return .deny
        }
        return .grant
    }

    static func notificationDecision(
        for securityOrigin: SecurityOriginDescriptor,
        expectedOrigin: PhoenixOrigin
    ) -> NotificationDecision {
        guard let candidate = url(for: securityOrigin), expectedOrigin.exactlyMatches(candidate) else {
            return .deny
        }
        return .grant
    }

    static func popupDecision(
        requestURL: URL?,
        sourceURL: URL?,
        expectedOrigin: PhoenixOrigin
    ) -> PopupDecision {
        guard let requestURL,
              let sourceURL,
              expectedOrigin.exactlyMatches(sourceURL) else { return .cancel }
        if requestURL.absoluteString == "about:blank" { return .allowManagedChild }
        if expectedOrigin.exactlyMatches(requestURL) { return .allowManagedChild }
        if requestURL.scheme?.lowercased() == nil {
            return .allowManagedChild
        }
        if safeToExternalize(requestURL) { return .externalize }
        return .cancel
    }

    static func safeToExternalize(_ url: URL) -> Bool {
        guard let scheme = url.scheme?.lowercased() else { return false }
        return ["http", "https", "mailto", "tel"].contains(scheme)
    }

    static func popupNavigationDecision(_ url: URL) -> PopupDecision {
        guard let scheme = url.scheme?.lowercased() else { return .cancel }
        switch scheme {
        case "about", "http", "https": return .allowManagedChild
        case "mailto", "tel": return .externalize
        default: return .cancel
        }
    }
}

enum PhoenixDownloadNaming {
    static func sanitizedFilename(_ suggestedFilename: String) -> String {
        let trimmed = suggestedFilename.trimmingCharacters(in: .whitespacesAndNewlines)
        let base = trimmed.isEmpty ? "download" : trimmed
        let invalidPunctuation = CharacterSet(charactersIn: "/:\\<>|?*")
        let pieces = base.unicodeScalars.map { scalar -> String in
            switch scalar.value {
            case 0..<32, 127:
                return "_"
            default:
                if invalidPunctuation.contains(scalar) || scalar == "\"" {
                    return "_"
                }
                return String(scalar)
            }
        }
        let collapsed = pieces.joined()
            .replacingOccurrences(of: "..", with: "_")
            .trimmingCharacters(in: CharacterSet(charactersIn: ". "))
        return collapsed.isEmpty ? "download" : collapsed
    }

    static func uniqueDestination(
        in directory: URL,
        suggestedFilename: String,
        fileExists: (URL) -> Bool
    ) -> URL {
        let safeName = sanitizedFilename(suggestedFilename)
        let extensionPart = (safeName as NSString).pathExtension
        let stem = (safeName as NSString).deletingPathExtension
        let baseStem = stem.isEmpty ? "download" : stem

        func candidate(_ index: Int?) -> URL {
            let filename: String
            if let index {
                filename = extensionPart.isEmpty
                    ? "\(baseStem) \(index)"
                    : "\(baseStem) \(index).\(extensionPart)"
            } else {
                filename = extensionPart.isEmpty ? baseStem : "\(baseStem).\(extensionPart)"
            }
            return directory.appendingPathComponent(filename)
        }

        let initial = candidate(nil)
        guard fileExists(initial) else { return initial }
        var suffix = 2
        while true {
            let proposed = candidate(suffix)
            if !fileExists(proposed) { return proposed }
            suffix += 1
        }
    }
}

struct SecurityOriginDescriptor: Equatable {
    let scheme: String
    let host: String
    let port: Int
}

enum MediaCaptureKind: Equatable {
    case camera
    case microphone
    case cameraAndMicrophone
    case display
    case unknown
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
    case invalidAttachedCleartextOrigin
    case missingModeSelection

    var errorDescription: String? {
        switch self {
        case .invalidOrigin: "Enter an HTTP or HTTPS origin without a path, credentials, query, or fragment."
        case .invalidPort: "Bundled Phoenix port must be between 1024 and 65535."
        case .bundledBinaryMissing: "The Phoenix sidecar is not present in this app bundle."
        case .bundledDataInUse: "Another Phoenix.app instance owns the bundled data directory."
        case .invalidAttachedCleartextOrigin: "HTTP attached origins are supported only for localhost or loopback addresses. Use HTTPS for remote hosts."
        case .missingModeSelection: "Choose whether Phoenix.app should connect to a managed deployment or start its bundled sidecar before connecting."
        }
    }
}

struct AttachedServerConfiguration: Equatable {
    let origin: PhoenixOrigin
}

struct BundledServerConfiguration: Equatable {
    let origin: PhoenixOrigin
    let executableURL: URL
    let runtimeRootURL: URL
    let dataDirectoryURL: URL
    let databaseURL: URL
    let logURL: URL
    let ownerLockURL: URL
    let rustLogLevel: String

    var publicEnvironment: [String: String] {
        [
            "HOME": runtimeRootURL.path,
            "CODEX_HOME": runtimeRootURL.appendingPathComponent(".codex", isDirectory: true).path,
            "PHOENIX_DATA_DIR": dataDirectoryURL.path,
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
        try loadCandidate(
            kind: defaults.string(forKey: PreferenceKey.serverMode)
                .flatMap(ServerModeKind.init(rawValue:)) ?? .attached,
            attachedOrigin: defaults.string(forKey: PreferenceKey.attachedOrigin) ?? defaultAttachedOrigin,
            bundledPort: defaults.integer(forKey: PreferenceKey.bundledPort),
            developmentBinaryOverride: defaults.string(forKey: PreferenceKey.bundledDevelopmentBinary) ?? "",
            rustLogLevel: defaults.string(forKey: PreferenceKey.rustLogLevel) ?? "phoenix_ide=info",
            bundle: bundle
        )
    }

    static func loadCandidate(
        kind: ServerModeKind,
        attachedOrigin: String,
        bundledPort: Int,
        developmentBinaryOverride: String,
        rustLogLevel: String,
        bundle: Bundle = .main
    ) throws -> ServerMode {
        switch kind {
        case .attached:
            let origin = try PhoenixOrigin(attachedOrigin)
            try validateAttachedOriginTransport(origin)
            return .attached(AttachedServerConfiguration(origin: origin))
        case .bundled:
            let port = bundledPort == 0 ? defaultBundledPort : bundledPort
            guard (1024...65535).contains(port) else { throw ConfigurationError.invalidPort }
            let root = try applicationSupportRoot()
            let executable = bundledExecutable(bundle: bundle, developmentBinaryOverride: developmentBinaryOverride)
            guard FileManager.default.isExecutableFile(atPath: executable.path) else {
                throw ConfigurationError.bundledBinaryMissing
            }
            let runtimeRoot = root.appendingPathComponent("sidecar-home", isDirectory: true)
            let dataDir = runtimeRoot.appendingPathComponent(".phoenix-ide", isDirectory: true)
            return .bundled(BundledServerConfiguration(
                origin: try PhoenixOrigin("http://127.0.0.1:\(port)"),
                executableURL: executable,
                runtimeRootURL: runtimeRoot,
                dataDirectoryURL: dataDir,
                databaseURL: dataDir.appendingPathComponent("phoenix.db"),
                logURL: dataDir.appendingPathComponent("phoenix.log"),
                ownerLockURL: dataDir.appendingPathComponent("owner.lock"),
                rustLogLevel: rustLogLevel
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

    private static func bundledExecutable(bundle: Bundle, developmentBinaryOverride: String) -> URL {
        #if DEBUG
        if !developmentBinaryOverride.isEmpty {
            return URL(fileURLWithPath: NSString(string: developmentBinaryOverride).expandingTildeInPath)
        }
        #endif
        return bundle.bundleURL.appendingPathComponent("Contents/Helpers/phoenix_ide")
    }

    private static func validateAttachedOriginTransport(_ origin: PhoenixOrigin) throws {
        guard origin.url.scheme?.lowercased() == "http" else { return }
        guard let host = origin.url.host?.lowercased(),
              host == "localhost" || host == "127.0.0.1" || host == "::1" else {
            throw ConfigurationError.invalidAttachedCleartextOrigin
        }
    }

    static func removeLegacyPlaintextSecret(defaults: UserDefaults = .standard) {
        // The old app wrote this directly to its preferences plist. It is deliberately
        // deleted rather than silently migrating a secret into a differently-owned app.
        defaults.removeObject(forKey: PreferenceKey.legacyAnthropicAPIKey)
        if let legacy = UserDefaults(suiteName: "com.scottopell.pa") {
            legacy.removeObject(forKey: PreferenceKey.legacyAnthropicAPIKey)
            legacy.synchronize()
        }
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

struct KeychainStore: SecretStore {
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

    func processEnvironment() throws -> [String: String] {
        var result: [String: String] = [:]
        for secret in ProviderSecret.allCases {
            if let value = try read(secret), !value.isEmpty {
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

enum KeychainError: LocalizedError, Equatable {
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
    case conversation(id: UUID)

    init?(url: URL) {
        guard url.scheme?.lowercased() == "phoenix" else { return nil }
        let action = url.host?.lowercased() ?? ""
        switch action {
        case "", "open": self = .open
        case "status", "debug": self = .status
        case "conversation":
            let segment = url.path.trimmingCharacters(in: CharacterSet(charactersIn: "/"))
            guard !segment.contains("/"), let id = UUID(uuidString: segment) else { return nil }
            self = .conversation(id: id)
        default: return nil
        }
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
    let instanceID: String?
    let localAccess: Bool
    let installationOwnership: InstallationOwnership

    enum CodingKeys: String, CodingKey {
        case build, network
        case instanceID = "instance_id"
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
