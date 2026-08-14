import Foundation
import Network
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

    var changedBundledSecrets: Bool {
        !savedSecrets.isEmpty || !deletedSecrets.isEmpty
    }
}

enum ReconnectIntent {
    case applySettings
    case statusReconnect
}

struct ConnectionReapplyDecision: Equatable {
    let requiresReconnect: Bool

    static func evaluate(
        currentMode: ServerMode?,
        currentState: ConnectionState,
        candidate: ServerMode,
        intent: ReconnectIntent
    ) -> ConnectionReapplyDecision {
        if intent == .statusReconnect {
            return ConnectionReapplyDecision(requiresReconnect: true)
        }
        guard currentMode == candidate else {
            return ConnectionReapplyDecision(requiresReconnect: true)
        }
        switch currentState {
        case .ready:
            return ConnectionReapplyDecision(requiresReconnect: false)
        default:
            return ConnectionReapplyDecision(requiresReconnect: true)
        }
    }
}


struct ServerReconnectRequest: Equatable {
    let candidate: ServerMode
    let requiresReconnect: Bool
    let forceRestart: Bool

    static func evaluate(
        currentMode: ServerMode?,
        currentState: ConnectionState,
        candidate: ServerMode,
        changedBundledSecrets: Bool,
        intent: ReconnectIntent = .applySettings
    ) -> ServerReconnectRequest {
        let reapply = ConnectionReapplyDecision.evaluate(
            currentMode: currentMode,
            currentState: currentState,
            candidate: candidate,
            intent: intent
        )
        let requiresForceRestart = candidate.kind == .bundled && changedBundledSecrets
        return ServerReconnectRequest(
            candidate: candidate,
            requiresReconnect: reapply.requiresReconnect || requiresForceRestart,
            forceRestart: requiresForceRestart
        )
    }
}

struct SettingsFeedback: Equatable {
    let statusMessage: String?
    let errorMessage: String?

    static let empty = SettingsFeedback(statusMessage: nil, errorMessage: nil)

    static func success(summary: SettingsPersistenceSummary) -> SettingsFeedback {
        SettingsFeedback(statusMessage: statusMessage(summary: summary), errorMessage: nil)
    }

    static func failure(_ error: Error) -> SettingsFeedback {
        SettingsFeedback(statusMessage: nil, errorMessage: error.localizedDescription)
    }

    static func statusMessage(summary: SettingsPersistenceSummary) -> String {
        var parts: [String] = []
        if !summary.savedSecrets.isEmpty {
            parts.append("Saved provider secrets to Keychain only as part of this Apply and Connect.")
        }
        if !summary.deletedSecrets.isEmpty {
            parts.append("Deleted cleared provider secrets from Keychain.")
        }
        if summary.requiresReconnect {
            parts.append("Saved settings now govern new connections.")
        } else {
            parts.append("Saved settings already match the saved configuration.")
        }
        return parts.joined(separator: " ")
    }
}

struct DeepLinkAuthDecision {
    static func shouldKeepQueuedConversationOnDeploymentAuthenticationStop(
        pendingConversationID: UUID?,
        stopWasForTransition: Bool
    ) -> Bool {
        pendingConversationID != nil && !stopWasForTransition
    }
}

struct SidecarLaunchEnvironment {
    static func executableSearchPath(inherited: String?) -> String {
        var entries = (inherited ?? "").split(separator: ":").map(String.init).filter { !$0.isEmpty }
        for path in ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
            if !entries.contains(path) { entries.append(path) }
        }
        return entries.joined(separator: ":")
    }

    static let safeInheritedKeys = ["SHELL", "USER", "LOGNAME", "SSH_AUTH_SOCK"]

    static func build(
        inherited: [String: String],
        userHome: URL,
        instanceID: UUID,
        publicEnvironment: [String: String],
        sidecarSecrets: [String: String]
    ) -> [String: String] {
        var environment = [
            "PATH": executableSearchPath(inherited: inherited["PATH"]),
            "TMPDIR": inherited["TMPDIR"] ?? NSTemporaryDirectory(),
            "LANG": inherited["LANG"] ?? "en_US.UTF-8",
            "HOME": userHome.path,
            "PHOENIX_INSTANCE_ID": instanceID.uuidString,
        ]
        for key in safeInheritedKeys {
            if let value = inherited[key], !value.isEmpty {
                environment[key] = value
            }
        }
        for (key, value) in publicEnvironment { environment[key] = value }
        for (key, value) in sidecarSecrets { environment[key] = value }
        return environment
    }
}

struct DeepLinkConversationRouteResponse: Decodable, Equatable {
    let id: String
    let slug: String?
}

struct DeepLinkConversationValidation {
    static func extractConversationID(fromRouteBody body: Any) -> UUID? {
        guard JSONSerialization.isValidJSONObject(body),
              let data = try? JSONSerialization.data(withJSONObject: body),
              let route = try? JSONDecoder().decode(DeepLinkConversationRouteResponse.self, from: data) else {
            return nil
        }
        return UUID(uuidString: route.id)
    }
}

struct QueuedDeepLinkAuthorityDecision {
    static func shouldRetain(pendingOrigin: PhoenixOrigin?, nextOrigin: PhoenixOrigin?) -> Bool {
        pendingOrigin != nil && pendingOrigin == nextOrigin
    }
}

struct DeepLinkNavigationDecision {
    static func validationResultIsCurrent(validatedID: UUID, pendingConversationID: UUID?) -> Bool {
        pendingConversationID == validatedID
    }

    static func shouldValidateQueuedConversation(
        pendingConversationID: UUID?,
        hasAuthenticatedPrimaryWebView: Bool,
        hasPrimaryWebView: Bool,
        hasConfiguredOrigin: Bool
    ) -> Bool {
        pendingConversationID != nil
            && hasAuthenticatedPrimaryWebView
            && hasPrimaryWebView
            && hasConfiguredOrigin
    }
}

struct DeepLinkValidationOutcome: Equatable {
    let shouldClearAuthenticationGate: Bool
    let shouldRetainPendingConversation: Bool
    let shouldReenterAuthentication: Bool

    static func evaluate(_ error: DeepLinkValidationError) -> DeepLinkValidationOutcome {
        switch error {
        case .authenticationRequired, .invalidHTTPStatus(401):
            return DeepLinkValidationOutcome(
                shouldClearAuthenticationGate: true,
                shouldRetainPendingConversation: true,
                shouldReenterAuthentication: true
            )
        default:
            return DeepLinkValidationOutcome(
                shouldClearAuthenticationGate: false,
                shouldRetainPendingConversation: false,
                shouldReenterAuthentication: false
            )
        }
    }
}

struct WebViewNavigationFailureDecision {
    static func shouldReport(_ error: Error) -> Bool {
        let failure = error as NSError
        return !(failure.domain == NSURLErrorDomain && failure.code == NSURLErrorCancelled)
    }
}

struct BrowserSurfaceOwnershipDecision {
    static func shouldCloseOwnedSurfaces(
        hasBrowserOperation: Bool,
        stateCanDisplayWebView: Bool
    ) -> Bool {
        hasBrowserOperation && !stateCanDisplayWebView
    }
}

struct LoopbackAddressPolicy {
    static func allowsCleartextAttachedOrigin(host: String?) -> Bool {
        guard let host else { return false }
        let normalized = host.trimmingCharacters(in: CharacterSet(charactersIn: "[]")).lowercased()
        guard !normalized.isEmpty else { return false }
        if normalized == "localhost" { return true }
        if normalized == "::1", IPv6Address(normalized)?.isLoopback == true { return true }
        if let ipv4 = IPv4Address(normalized) {
            return ipv4.rawValue.first == 127
        }
        return false
    }

    static func bundledBindAddressIsLoopback(_ bindAddress: String) -> Bool {
        guard let host = hostComponent(fromBindAddress: bindAddress) else { return false }
        return allowsCleartextAttachedOrigin(host: host)
    }

    private static func hostComponent(fromBindAddress bindAddress: String) -> String? {
        if bindAddress.hasPrefix("[") {
            guard let end = bindAddress.firstIndex(of: "]") else { return nil }
            return String(bindAddress[bindAddress.index(after: bindAddress.startIndex)..<end])
        }
        if let lastColon = bindAddress.lastIndex(of: ":") {
            let candidate = String(bindAddress[..<lastColon])
            if candidate.contains(":") {
                return bindAddress
            }
            return candidate
        }
        return bindAddress
    }
}

enum BundledSecretState: Equatable {
    case unloaded
    case preserveUnloaded
    case loaded(String?)

    var value: String? {
        switch self {
        case .unloaded, .preserveUnloaded: nil
        case .loaded(let value): value
        }
    }

    var writeValue: String? {
        switch self {
        case .loaded(nil):
            return ""
        case .preserveUnloaded:
            return nil
        case .unloaded, .loaded:
            return value
        }
    }

    var draftValue: String {
        switch self {
        case .unloaded, .preserveUnloaded, .loaded(nil): ""
        case .loaded(let value?): value
        }
    }
}

struct BundledSecretAccessPolicy {
    static func shouldReadKeychain(for mode: PendingServerModeKind?) -> Bool {
        mode == .bundled
    }

    static func shouldWriteKeychain(for mode: PendingServerModeKind?) -> Bool {
        mode == .bundled
    }
}

struct LogRecordBounds {
    let maxRecordBytes: Int

    func boundedRecord(_ data: Data) -> Data {
        guard maxRecordBytes > 0, data.count > maxRecordBytes else { return data }
        return Data(data.suffix(maxRecordBytes))
    }
}

private extension DeploymentInfo {
    func originExactlyMatches(_ origin: PhoenixOrigin) -> Bool {
        origin.exactlyMatches(originURL)
    }

    var originURL: URL {
        let originString = currentMode?.webOrigin ?? networkOriginString
        return URL(string: originString) ?? fallbackNetworkOriginURL
    }

    var networkOriginString: String {
        let hostPort = network.bindAddress
        let scheme = network.tls.enabled ? "https" : "http"
        if hostPort.hasPrefix("[") {
            return "\(scheme)://\(hostPort)"
        }
        let parts = hostPort.split(separator: ":", maxSplits: 1).map(String.init)
        if parts.count == 2, parts[0].contains(":") {
            return "\(scheme)://[\(parts[0])]:\(parts[1])"
        }
        return "\(scheme)://\(hostPort)"
    }

    var fallbackNetworkOriginURL: URL {
        URL(string: networkOriginString)!
    }
}

struct ReopenDecision {
    static func shouldShowMainWindow(mainWindowIsVisible: Bool) -> Bool {
        !mainWindowIsVisible
    }
}
enum FirstRunDecision {
    static func shouldOpenSettings(hasSavedModeSelection: Bool) -> Bool {
        !hasSavedModeSelection
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

protocol PackagedSidecarSecretProvider {
    func sidecarEnvironment() throws -> [String: String]
}

struct PersistedPreferenceSnapshot: Equatable {
    let serverMode: String?
    let attachedOrigin: String?
    let bundledPort: Int?
    let developmentBinaryOverride: String?
    let rustLogLevel: String?
}

private struct SecretWritePlan {
    enum Outcome: Equatable {
        case preserveExisting
        case write(String)
    }

    enum Source: Equatable {
        case preservedSnapshot
        case explicitDelete
        case explicitValue
    }

    let persistedState: BundledSecretState
    let rollbackValue: String?
    let outcome: Outcome
    let source: Source

    var writesKeychain: Bool {
        switch outcome {
        case .preserveExisting: false
        case .write: true
        }
    }

    var savedSecretValue: String? {
        guard case .write(let value) = outcome else { return nil }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        return trimmed.isEmpty ? nil : trimmed
    }

    var deletesSecret: Bool {
        source == .explicitDelete
    }
}

struct PersistedSettingsSnapshot: Equatable {
    let preferences: PersistedPreferenceSnapshot
    let secrets: [ProviderSecret: BundledSecretState]

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
            anthropicKey: (secrets[.anthropicAPIKey] ?? .unloaded).draftValue,
            openAIKey: (secrets[.openAIAPIKey] ?? .unloaded).draftValue
        )
    }

    func nonsecretDraft() -> SettingsDraft {
        var draft = draft()
        draft.anthropicKey = ""
        draft.openAIKey = ""
        return draft
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

    private func persistedPreferences() -> PersistedPreferenceSnapshot {
        PersistedPreferenceSnapshot(
            serverMode: defaults.string(forKey: PreferenceKey.serverMode),
            attachedOrigin: defaults.string(forKey: PreferenceKey.attachedOrigin),
            bundledPort: defaults.object(forKey: PreferenceKey.bundledPort) as? Int,
            developmentBinaryOverride: defaults.string(forKey: PreferenceKey.bundledDevelopmentBinary),
            rustLogLevel: defaults.string(forKey: PreferenceKey.rustLogLevel)
        )
    }

    func loadDraft() throws -> DraftLoadResult {
        let snapshot = try persistedSnapshot()
        let hasSavedModeSelection = snapshot.preferences.serverMode.flatMap(ServerModeKind.init(rawValue:)) != nil
        return DraftLoadResult(draft: snapshot.draft(), hasSavedModeSelection: hasSavedModeSelection)
    }

    func loadConnectionDraft() -> DraftLoadResult {
        let preferences = persistedPreferences()
        let snapshot = PersistedSettingsSnapshot(preferences: preferences, secrets: [:])
        return DraftLoadResult(
            draft: snapshot.nonsecretDraft(),
            hasSavedModeSelection: preferences.serverMode.flatMap(ServerModeKind.init(rawValue:)) != nil
        )
    }

    func persistedSnapshot() throws -> PersistedSettingsSnapshot {
        let preferences = persistedPreferences()
        let mode = preferences.serverMode.flatMap(ServerModeKind.init(rawValue:)).map(PendingServerModeKind.init)
        var secrets: [ProviderSecret: BundledSecretState] = [:]
        if BundledSecretAccessPolicy.shouldReadKeychain(for: mode) {
            for secret in ProviderSecret.allCases {
                secrets[secret] = .loaded(try keychain.read(secret))
            }
        } else {
            for secret in ProviderSecret.allCases {
                secrets[secret] = .unloaded
            }
        }
        return PersistedSettingsSnapshot(preferences: preferences, secrets: secrets)
    }

    func loadBundledSecrets(
        into draft: inout SettingsDraft,
        appliedSnapshot: PersistedSettingsSnapshot
    ) throws -> PersistedSettingsSnapshot {
        var secrets: [ProviderSecret: BundledSecretState] = [:]
        for secret in ProviderSecret.allCases {
            let value = try keychain.read(secret)
            secrets[secret] = .loaded(value)
            switch secret {
            case .anthropicAPIKey: draft.anthropicKey = value ?? ""
            case .openAIAPIKey: draft.openAIKey = value ?? ""
            }
        }
        return PersistedSettingsSnapshot(preferences: appliedSnapshot.preferences, secrets: secrets)
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
        let keychainWritesActive = BundledSecretAccessPolicy.shouldWriteKeychain(for: draft.mode)
        let plans = try secretWritePlans(for: draft, previous: priorAppliedSnapshot)

        let savedSecrets = keychainWritesActive ? ProviderSecret.allCases.compactMap { secret -> ProviderSecret? in
            guard let value = plans[secret]?.savedSecretValue, !value.isEmpty else { return nil }
            return secret
        } : []
        let deletedSecrets = keychainWritesActive ? ProviderSecret.allCases.compactMap { secret -> ProviderSecret? in
            plans[secret]?.deletesSecret == true ? secret : nil
        } : []

        do {
            try apply(draft: draft, plans: plans)
        } catch {
            if let rollbackFailure = rollback(to: priorAppliedSnapshot, plans: plans) {
                throw SettingsPersistenceError.applyFailedWithRollbackFailure(cause: error, rollbackFailure: rollbackFailure)
            }
            throw error
        }

        let after = PersistedSettingsSnapshot(
            preferences: PersistedPreferenceSnapshot(
                serverMode: mode.persistedKind.rawValue,
                attachedOrigin: draft.attachedOrigin,
                bundledPort: draft.bundledPort,
                developmentBinaryOverride: draft.developmentBinaryOverride,
                rustLogLevel: draft.rustLogLevel
            ),
            secrets: Dictionary(uniqueKeysWithValues: ProviderSecret.allCases.map { secret in
                guard keychainWritesActive else { return (secret, priorAppliedSnapshot.secrets[secret] ?? .unloaded) }
                return (secret, plans[secret]?.persistedState ?? .unloaded)
            })
        )
        return (candidate, SettingsPersistenceSummary(
            requiresReconnect: priorAppliedSnapshot.draft() != after.draft(),
            savedSecrets: savedSecrets,
            deletedSecrets: deletedSecrets
        ), after)
    }

    private func apply(draft: SettingsDraft, plans: [ProviderSecret: SecretWritePlan]) throws {
        writePreferences(for: draft)
        guard BundledSecretAccessPolicy.shouldWriteKeychain(for: draft.mode) else { return }
        for secret in orderedSecretsForApply(plans: plans) {
            guard let plan = plans[secret] else { continue }
            switch plan.outcome {
            case .preserveExisting:
                continue
            case .write(let value):
                try keychain.write(value, for: secret)
            }
        }
    }

    private func writePreferences(for draft: SettingsDraft) {
        defaults.set(draft.mode?.persistedKind.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set(draft.attachedOrigin, forKey: PreferenceKey.attachedOrigin)
        defaults.set(draft.bundledPort, forKey: PreferenceKey.bundledPort)
        defaults.set(draft.developmentBinaryOverride, forKey: PreferenceKey.bundledDevelopmentBinary)
        defaults.set(draft.rustLogLevel, forKey: PreferenceKey.rustLogLevel)
    }

    private func orderedSecretsForApply(plans: [ProviderSecret: SecretWritePlan]) -> [ProviderSecret] {
        ProviderSecret.allCases.sorted { lhs, rhs in
            let lhsRank = applyOrderRank(for: plans[lhs])
            let rhsRank = applyOrderRank(for: plans[rhs])
            if lhsRank != rhsRank { return lhsRank < rhsRank }
            return lhs.rawValue < rhs.rawValue
        }
    }

    private func applyOrderRank(for plan: SecretWritePlan?) -> Int {
        guard let plan else { return 3 }
        switch plan.source {
        case .explicitDelete:
            return 0
        case .explicitValue:
            return 1
        case .preservedSnapshot:
            return 2
        }
    }

    private func rollback(to snapshot: PersistedSettingsSnapshot, plans: [ProviderSecret: SecretWritePlan]) -> Error? {
        var rollbackFailures: [Error] = []
        restorePreferences(snapshot.preferences)
        let mode = snapshot.preferences.serverMode.flatMap(ServerModeKind.init(rawValue:)).map(PendingServerModeKind.init)
        guard BundledSecretAccessPolicy.shouldWriteKeychain(for: mode) || plans.values.contains(where: { $0.writesKeychain }) else { return nil }
        for secret in ProviderSecret.allCases {
            guard plans[secret]?.writesKeychain == true else { continue }
            do {
                try keychain.write(plans[secret]?.rollbackValue ?? snapshot.secrets[secret]?.value ?? "", for: secret)
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

    private func secretWritePlans(
        for draft: SettingsDraft,
        previous: PersistedSettingsSnapshot
    ) throws -> [ProviderSecret: SecretWritePlan] {
        guard BundledSecretAccessPolicy.shouldWriteKeychain(for: draft.mode) else { return [:] }
        return try Dictionary(uniqueKeysWithValues: ProviderSecret.allCases.map { secret in
            (secret, try secretWritePlan(for: secret, draft: draft, previous: previous))
        })
    }

    private func secretWritePlan(
        for secret: ProviderSecret,
        draft: SettingsDraft,
        previous: PersistedSettingsSnapshot
    ) throws -> SecretWritePlan {
        let trimmed = secretValue(in: draft, for: secret).trimmingCharacters(in: .whitespacesAndNewlines)
        let previousState = previous.secrets[secret] ?? .unloaded
        let switchingToBundled = draft.mode == .bundled
            && previous.preferences.serverMode.flatMap(ServerModeKind.init(rawValue:)) != .bundled
        let previousValue = try previousSecretValue(for: secret, previousState: previousState)

        if trimmed.isEmpty {
            switch previousState {
            case .unloaded, .preserveUnloaded:
                if switchingToBundled {
                    return SecretWritePlan(
                        persistedState: .unloaded,
                        rollbackValue: previousValue,
                        outcome: .write(previousValue ?? ""),
                        source: .preservedSnapshot
                    )
                }
                return SecretWritePlan(
                    persistedState: .unloaded,
                    rollbackValue: previousValue,
                    outcome: .preserveExisting,
                    source: .preservedSnapshot
                )
            case .loaded(let value):
                guard value != nil else {
                    return SecretWritePlan(
                        persistedState: .loaded(nil),
                        rollbackValue: nil,
                        outcome: .preserveExisting,
                        source: .preservedSnapshot
                    )
                }
                return SecretWritePlan(
                    persistedState: .loaded(nil),
                    rollbackValue: previousValue,
                    outcome: .write(""),
                    source: .explicitDelete
                )
            }
        }

        if case .loaded(let previous?) = previousState,
           previous.trimmingCharacters(in: .whitespacesAndNewlines) == trimmed {
            return SecretWritePlan(
                persistedState: .loaded(previous),
                rollbackValue: previous,
                outcome: .preserveExisting,
                source: .preservedSnapshot
            )
        }

        return SecretWritePlan(
            persistedState: .loaded(trimmed),
            rollbackValue: previousValue,
            outcome: .write(trimmed),
            source: .explicitValue
        )
    }

    private func previousSecretValue(for secret: ProviderSecret, previousState: BundledSecretState) throws -> String? {
        switch previousState {
        case .loaded(let value):
            return value
        case .unloaded, .preserveUnloaded:
            return try keychain.read(secret)
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
        if securityOrigin.port > 0 {
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
        if requestURL.scheme?.lowercased() == "blob",
           requestURL.absoluteString.hasPrefix("blob:\(expectedOrigin.url.absoluteString)") {
            return .allowManagedChild
        }
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

    static func popupNavigationDecision(_ url: URL, expectedOrigin: PhoenixOrigin) -> PopupDecision {
        if url.absoluteString == "about:blank" || expectedOrigin.exactlyMatches(url) {
            return .allowManagedChild
        }
        if url.scheme?.lowercased() == "blob",
           url.absoluteString.hasPrefix("blob:\(expectedOrigin.url.absoluteString)") {
            return .allowManagedChild
        }
        guard let scheme = url.scheme?.lowercased() else { return .cancel }
        switch scheme {
        case "http", "https", "mailto", "tel": return .externalize
        default: return .cancel
        }
    }
}

enum PhoenixNavigationResponseDecision: Equatable {
    case allow
    case download
    case externalize(URL)
    case cancel
}

enum PhoenixNavigationResponsePolicy {
    static func decide(
        role: BrowserSurfaceRole,
        responseURL: URL?,
        canShowMIMEType: Bool,
        expectedOrigin: PhoenixOrigin,
        userActivated: Bool = false
    ) -> PhoenixNavigationResponseDecision {
        guard let responseURL else { return .cancel }
        if responseURL.scheme?.lowercased() == "blob",
           responseURL.absoluteString.hasPrefix("blob:\(expectedOrigin.url.absoluteString)") {
            return .allow
        }
        guard expectedOrigin.exactlyMatches(responseURL) else {
            if (role == .authPopup || userActivated), PhoenixWebViewPolicy.safeToExternalize(responseURL) {
                return .externalize(responseURL)
            }
            return .cancel
        }
        return canShowMIMEType ? .allow : .download
    }
}

enum PhoenixDownloadPolicy {
    static func shouldAccept(responseURL: URL?, canShowMIMEType: Bool, expectedOrigin: PhoenixOrigin) -> Bool {
        guard !canShowMIMEType, let responseURL else { return false }
        return expectedOrigin.exactlyMatches(responseURL)
    }
}

enum PhoenixDownloadNaming {
    static let maximumComponentBytes = 255
    static let reservedCollisionSuffixBytes = 24

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
        return boundedFilename(collapsed.isEmpty ? "download" : collapsed, collisionSuffix: "")
    }

    static func collisionSafeFilename(_ sanitizedFilename: String, collisionIndex: Int?) -> String {
        let suffix = collisionIndex.map { " (\($0))" } ?? ""
        return boundedFilename(sanitizedFilename, collisionSuffix: suffix)
    }

    private static func boundedFilename(_ filename: String, collisionSuffix: String) -> String {
        let value = filename as NSString
        let rawExtension = value.pathExtension
        let rawStem = value.deletingPathExtension.isEmpty ? "download" : value.deletingPathExtension
        let suffixBudget = max(reservedCollisionSuffixBytes, collisionSuffix.utf8.count)
        let contentBudget = maximumComponentBytes - suffixBudget
        let extensionWithDot = rawExtension.isEmpty ? "" : ".\(rawExtension)"
        let minimumStemBytes = "download".utf8.count
        let boundedExtension = utf8Prefix(extensionWithDot, maximumBytes: max(0, contentBudget - minimumStemBytes))
        let stemBudget = max(1, contentBudget - boundedExtension.utf8.count)
        let boundedStem = utf8Prefix(rawStem, maximumBytes: stemBudget)
        let usableStem = boundedStem.isEmpty ? utf8Prefix("download", maximumBytes: stemBudget) : boundedStem
        return usableStem + collisionSuffix + boundedExtension
    }

    private static func utf8Prefix(_ value: String, maximumBytes: Int) -> String {
        guard maximumBytes > 0 else { return "" }
        var result = ""
        var byteCount = 0
        for character in value {
            let bytes = String(character).utf8.count
            guard byteCount + bytes <= maximumBytes else { break }
            result.append(character)
            byteCount += bytes
        }
        return result
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
              components.port.map({ (1...65_535).contains($0) }) ?? true,
              components.path.isEmpty || components.path == "/" else {
            throw ConfigurationError.invalidOrigin
        }
        components.scheme = scheme
        components.path = ""
        guard let normalized = components.url else { throw ConfigurationError.invalidOrigin }
        url = normalized
    }

    private func canonicalHost(_ host: String) -> String {
        let unbracketed = host.trimmingCharacters(in: CharacterSet(charactersIn: "[]"))
        var address = in6_addr()
        if inet_pton(AF_INET6, unbracketed, &address) == 1 {
            var buffer = [CChar](repeating: 0, count: Int(INET6_ADDRSTRLEN))
            return withUnsafePointer(to: &address) { pointer in
                inet_ntop(AF_INET6, pointer, &buffer, socklen_t(INET6_ADDRSTRLEN)).map(String.init(cString:))
            } ?? unbracketed.lowercased()
        }
        return unbracketed.lowercased()
    }

    var canonicalStorageKey: String {
        let components = URLComponents(url: url, resolvingAgainstBaseURL: false)!
        let scheme = components.scheme!.lowercased()
        let host = canonicalHost(components.host!)
        let port = components.port ?? (scheme == "https" ? 443 : 80)
        return "\(scheme)://\(host):\(port)"
    }

    static func == (lhs: PhoenixOrigin, rhs: PhoenixOrigin) -> Bool {
        lhs.canonicalStorageKey == rhs.canonicalStorageKey
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
            && lhs.host.map(canonicalHost) == rhs.host.map(canonicalHost)
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
    let phoenixLogURL: URL
    let launcherLogURL: URL
    let ownerLockURL: URL
    let rustLogLevel: String

    static let launcherLogMaxBytes: Int64 = 512 * 1024

    var publicEnvironment: [String: String] {
        [
            "CODEX_HOME": runtimeRootURL.appendingPathComponent(".codex", isDirectory: true).path,
            "PHOENIX_DATA_DIR": dataDirectoryURL.path,
            "PHOENIX_STATE_DIR": dataDirectoryURL.path,
            "PHOENIX_TMP_DIR": runtimeRootURL.appendingPathComponent("tmp", isDirectory: true).appendingPathComponent("phoenix-ide", isDirectory: true).path,
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
        guard let persistedModeRaw = defaults.string(forKey: PreferenceKey.serverMode),
              let persistedMode = ServerModeKind(rawValue: persistedModeRaw) else {
            throw ConfigurationError.missingModeSelection
        }
        let persistedBundledPort = defaults.object(forKey: PreferenceKey.bundledPort) as? Int
        return try loadCandidate(
            kind: persistedMode,
            attachedOrigin: defaults.string(forKey: PreferenceKey.attachedOrigin) ?? defaultAttachedOrigin,
            bundledPort: persistedBundledPort ?? defaultBundledPort,
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
            guard (1024...65535).contains(bundledPort) else { throw ConfigurationError.invalidPort }
            let port = bundledPort
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
                phoenixLogURL: dataDir.appendingPathComponent("phoenix.log"),
                launcherLogURL: dataDir.appendingPathComponent("launcher.log"),
                ownerLockURL: runtimeRoot.appendingPathComponent("owner.lock"),
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
        guard LoopbackAddressPolicy.allowsCleartextAttachedOrigin(host: origin.url.host) else {
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

struct KeychainStore: SecretStore, PackagedSidecarSecretProvider {
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

    func sidecarEnvironment() throws -> [String: String] {
        try processEnvironment()
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

enum DeepLinkValidationError: LocalizedError, Equatable {
    case noAuthenticatedWebView
    case authenticationRequired
    case invalidHTTPStatus(Int)
    case decoding(String)
    case conversationMissing(UUID)

    var errorDescription: String? {
        switch self {
        case .noAuthenticatedWebView:
            "Open Phoenix and sign in before opening a conversation deep link."
        case .authenticationRequired:
            "Phoenix needs you to sign in before it can validate that conversation deep link."
        case .invalidHTTPStatus(let status):
            "Conversation deep link validation failed with HTTP status \(status)."
        case .decoding(let message):
            "Conversation deep link validation could not decode the Phoenix response: \(message)"
        case .conversationMissing(let id):
            "Phoenix could not find conversation \(id.uuidString.lowercased()) at the configured origin."
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
    let currentMode: CurrentModeInfo?
    let instanceID: String?
    let localAccess: Bool
    let installationOwnership: InstallationOwnership

    enum CodingKeys: String, CodingKey {
        case build, network
        case currentMode = "current_mode"
        case instanceID = "instance_id"
        case localAccess = "local_access"
        case installationOwnership = "installation_ownership"
    }
}

struct CurrentModeInfo: Codable, Equatable {
    let serverMode: String?
    let webOrigin: String?

    enum CodingKeys: String, CodingKey {
        case serverMode = "server_mode"
        case webOrigin = "web_origin"
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
