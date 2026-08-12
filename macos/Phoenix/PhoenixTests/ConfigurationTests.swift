import XCTest
@testable import PhoenixMacCore

private final class ScriptedSecretStore: SecretStore, PackagedSidecarSecretProvider {
    enum ReadStep {
        case succeed(String?)
        case fail(KeychainError)
    }

    enum WriteStep {
        case succeed(ProviderSecret)
        case fail(ProviderSecret, KeychainError)

        var secret: ProviderSecret {
            switch self {
            case .succeed(let secret), .fail(let secret, _): secret
            }
        }
    }

    private(set) var values: [ProviderSecret: String?]
    private var scriptedReads: [ProviderSecret: [ReadStep]]
    private var scriptedWrites: [WriteStep]

    init(
        values: [ProviderSecret: String?] = [:],
        scriptedReads: [ProviderSecret: [ReadStep]] = [:],
        scriptedWrites: [WriteStep] = []
    ) {
        self.values = Dictionary(uniqueKeysWithValues: ProviderSecret.allCases.map { secret in
            (secret, values[secret] ?? nil)
        })
        self.scriptedReads = scriptedReads
        self.scriptedWrites = scriptedWrites
    }

    func read(_ secret: ProviderSecret) throws -> String? {
        if var steps = scriptedReads[secret], !steps.isEmpty {
            let step = steps.removeFirst()
            scriptedReads[secret] = steps
            switch step {
            case .succeed(let value):
                return value
            case .fail(let error):
                throw error
            }
        }
        return values[secret] ?? nil
    }

    func write(_ value: String, for secret: ProviderSecret) throws {
        if !scriptedWrites.isEmpty {
            let step = scriptedWrites.removeFirst()
            precondition(step.secret == secret, "Expected scripted write for \(step.secret), got \(secret)")
            switch step {
            case .succeed:
                break
            case .fail(_, let error):
                throw error
            }
        }
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        values[secret] = trimmed.isEmpty ? nil : trimmed
    }

    func processEnvironment() throws -> [String: String] {
        Dictionary(uniqueKeysWithValues: ProviderSecret.allCases.compactMap { secret in
            guard let value = values[secret] ?? nil else { return nil }
            return (secret.environmentKey, value)
        })
    }

    func sidecarEnvironment() throws -> [String: String] {
        try processEnvironment()
    }
}

final class ConfigurationTests: XCTestCase {
    func testOriginNormalizesAndMatchesExactEffectiveOrigin() throws {
        let origin = try PhoenixOrigin("HTTPS://Example.COM/")
        XCTAssertTrue(origin.exactlyMatches(try XCTUnwrap(URL(string: "https://example.com/path"))))
        XCTAssertTrue(origin.exactlyMatches(try XCTUnwrap(URL(string: "https://EXAMPLE.com:443/other"))))
        XCTAssertFalse(origin.exactlyMatches(try XCTUnwrap(URL(string: "https://example.com.evil.test/"))))
        XCTAssertFalse(origin.exactlyMatches(try XCTUnwrap(URL(string: "http://example.com/"))))
        XCTAssertFalse(origin.exactlyMatches(try XCTUnwrap(URL(string: "https://example.com:8443/"))))
    }

    func testOriginRejectsValuesThatAreNotCanonicalOrigins() {
        for invalid in [
            "example.com",
            "ftp://example.com",
            "https://user:password@example.com",
            "https://example.com/a/path",
            "https://example.com?query=1",
            "https://example.com/#fragment",
        ] {
            XCTAssertThrowsError(try PhoenixOrigin(invalid), invalid)
        }
    }

    func testSecurityOriginURLBuilderBracketsIPv6Hosts() throws {
        let built = try XCTUnwrap(PhoenixWebViewPolicy.url(for: SecurityOriginDescriptor(scheme: "https", host: "::1", port: 8031)))
        XCTAssertEqual(built.absoluteString, "https://[::1]:8031")
    }

    func testMediaCapturePolicyAllowsOnlyExactOriginMicrophone() throws {
        let expected = try PhoenixOrigin("https://[::1]:8031")
        let descriptor = SecurityOriginDescriptor(scheme: "https", host: "::1", port: 8031)
        XCTAssertEqual(
            PhoenixWebViewPolicy.mediaCaptureDecision(for: descriptor, captureType: .microphone, expectedOrigin: expected),
            .grant
        )
        XCTAssertEqual(
            PhoenixWebViewPolicy.mediaCaptureDecision(for: descriptor, captureType: .camera, expectedOrigin: expected),
            .deny
        )
        XCTAssertEqual(
            PhoenixWebViewPolicy.mediaCaptureDecision(for: descriptor, captureType: .cameraAndMicrophone, expectedOrigin: expected),
            .deny
        )
        XCTAssertEqual(
            PhoenixWebViewPolicy.mediaCaptureDecision(
                for: SecurityOriginDescriptor(scheme: "https", host: "example.com", port: 8031),
                captureType: .microphone,
                expectedOrigin: expected
            ),
            .deny
        )
    }

    func testNotificationPolicyRequiresExactVerifiedOrigin() throws {
        let expected = try PhoenixOrigin("https://phoenix.example.test")
        XCTAssertEqual(
            PhoenixWebViewPolicy.notificationDecision(
                for: SecurityOriginDescriptor(scheme: "https", host: "phoenix.example.test", port: 443),
                expectedOrigin: expected
            ),
            .grant
        )
        XCTAssertEqual(
            PhoenixWebViewPolicy.notificationDecision(
                for: SecurityOriginDescriptor(scheme: "https", host: "other.example.test", port: 443),
                expectedOrigin: expected
            ),
            .deny
        )
    }

    func testPopupPolicyKeepsSameOriginAndAboutBlankManaged() throws {
        let expected = try PhoenixOrigin("https://phoenix.example.test")
        XCTAssertEqual(
            PhoenixWebViewPolicy.popupDecision(
                requestURL: URL(string: "about:blank"),
                sourceURL: URL(string: "https://phoenix.example.test/login"),
                expectedOrigin: expected
            ),
            .allowManagedChild
        )
        XCTAssertEqual(
            PhoenixWebViewPolicy.popupDecision(
                requestURL: URL(string: "https://phoenix.example.test/auth/popup"),
                sourceURL: URL(string: "https://phoenix.example.test/login"),
                expectedOrigin: expected
            ),
            .allowManagedChild
        )
    }

    func testPopupPolicyRejectsAboutBlankFromUntrustedSource() throws {
        let expected = try PhoenixOrigin("https://phoenix.example.test")
        XCTAssertEqual(
            PhoenixWebViewPolicy.popupDecision(
                requestURL: URL(string: "about:blank"),
                sourceURL: URL(string: "https://other.example.test"),
                expectedOrigin: expected
            ),
            .cancel
        )
        XCTAssertEqual(
            PhoenixWebViewPolicy.popupDecision(
                requestURL: URL(string: "about:blank"),
                sourceURL: nil,
                expectedOrigin: expected
            ),
            .cancel
        )
    }

    func testPopupPolicyExternalizesOnlySafeCrossOriginURLs() throws {
        let expected = try PhoenixOrigin("https://phoenix.example.test")
        XCTAssertEqual(
            PhoenixWebViewPolicy.popupDecision(
                requestURL: URL(string: "https://accounts.example.com/oauth"),
                sourceURL: URL(string: "https://phoenix.example.test/login"),
                expectedOrigin: expected
            ),
            .externalize
        )
        XCTAssertEqual(
            PhoenixWebViewPolicy.popupDecision(
                requestURL: URL(string: "file:///tmp/secret"),
                sourceURL: URL(string: "https://phoenix.example.test/login"),
                expectedOrigin: expected
            ),
            .cancel
        )
    }

    func testAuthPopupNavigationAllowsWebOAuthButRejectsPrivilegedSchemes() {
        XCTAssertEqual(PhoenixWebViewPolicy.popupNavigationDecision(URL(string: "https://accounts.example.test/oauth")!), .allowManagedChild)
        XCTAssertEqual(PhoenixWebViewPolicy.popupNavigationDecision(URL(string: "about:blank")!), .allowManagedChild)
        XCTAssertEqual(PhoenixWebViewPolicy.popupNavigationDecision(URL(string: "mailto:help@example.test")!), .externalize)
        XCTAssertEqual(PhoenixWebViewPolicy.popupNavigationDecision(URL(string: "file:///tmp/secret")!), .cancel)
    }

    func testDownloadNamingSanitizesDangerousSuggestedNames() {
        XCTAssertEqual(PhoenixDownloadNaming.sanitizedFilename(" ../../quarterly?.pdf "), "____quarterly_.pdf")
        XCTAssertEqual(PhoenixDownloadNaming.sanitizedFilename("   "), "download")
        XCTAssertEqual(PhoenixDownloadNaming.sanitizedFilename("report\u{0000}.csv"), "report_.csv")
    }

    func testDownloadNamingPicksCollisionSafeSuffixWithoutOverwriting() {
        let directory = URL(fileURLWithPath: "/tmp/downloads", isDirectory: true)
        let existing: Set<String> = ["report.pdf", "report 2.pdf", "report 3.pdf"]
        let destination = PhoenixDownloadNaming.uniqueDestination(
            in: directory,
            suggestedFilename: "report.pdf",
            fileExists: { existing.contains($0.lastPathComponent) }
        )
        XCTAssertEqual(destination.lastPathComponent, "report 4.pdf")
    }

    func testLoadRejectsMissingBundledPortByUsingDefaultOnlyWhenPreferenceAbsent() throws {
        let suite = "ConfigurationStore.load.default-port.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.bundled.rawValue, forKey: PreferenceKey.serverMode)

        let persistence = SettingsPersistence(defaults: defaults, keychain: ScriptedSecretStore(), bundle: .main)
        let loaded = persistence.loadConnectionDraft()

        XCTAssertTrue(loaded.hasSavedModeSelection)
        XCTAssertEqual(loaded.draft.mode, .bundled)
        XCTAssertEqual(loaded.draft.bundledPort, ConfigurationStore.defaultBundledPort)
    }

    func testLoadCandidateRejectsExplicitBundledPortZero() {
        XCTAssertThrowsError(
            try ConfigurationStore.loadCandidate(
                kind: .bundled,
                attachedOrigin: ConfigurationStore.defaultAttachedOrigin,
                bundledPort: 0,
                developmentBinaryOverride: "/bin/sh",
                rustLogLevel: "phoenix_ide=info"
            )
        ) { error in
            XCTAssertEqual(error as? ConfigurationError, .invalidPort)
        }
    }

    func testLoadCandidateRejectsRemoteCleartextAttachedOrigin() {
        XCTAssertThrowsError(
            try ConfigurationStore.loadCandidate(
                kind: .attached,
                attachedOrigin: "http://phoenix.example.test:8031",
                bundledPort: ConfigurationStore.defaultBundledPort,
                developmentBinaryOverride: "",
                rustLogLevel: "phoenix_ide=info"
            )
        ) { error in
            XCTAssertEqual(error as? ConfigurationError, .invalidAttachedCleartextOrigin)
        }
    }

    func testConnectionDraftRedactsSecretsButPreservesSavedMode() throws {
        let suite = "SettingsPersistence.connection-load.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.bundled.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set(9420, forKey: PreferenceKey.bundledPort)
        let keychain = ScriptedSecretStore(values: [
            .anthropicAPIKey: "anthropic-secret",
            .openAIAPIKey: "openai-secret",
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)

        let loaded = persistence.loadConnectionDraft()

        XCTAssertTrue(loaded.hasSavedModeSelection)
        XCTAssertEqual(loaded.draft.mode, .bundled)
        XCTAssertEqual(loaded.draft.bundledPort, 9420)
        XCTAssertEqual(loaded.draft.anthropicKey, "")
        XCTAssertEqual(loaded.draft.openAIKey, "")
    }

    func testPersistedSnapshotMarksAttachedSecretsUnloadedInsteadOfEmpty() throws {
        let suite = "SettingsPersistence.snapshot.attached-unloaded.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.attached.rawValue, forKey: PreferenceKey.serverMode)
        let keychain = ScriptedSecretStore(values: [
            .anthropicAPIKey: "preserved-secret",
            .openAIAPIKey: "other-secret",
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)

        let snapshot = try persistence.persistedSnapshot()

        XCTAssertEqual(snapshot.secrets[.anthropicAPIKey], .unloaded)
        XCTAssertEqual(snapshot.secrets[.openAIAPIKey], .unloaded)
    }

    func testBundledApplyPreservesUnloadedSecretWhenDraftLeavesFieldBlank() throws {
        let suite = "SettingsPersistence.persist.unloaded-secret.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = ScriptedSecretStore(values: [
            .anthropicAPIKey: "preserved-secret",
            .openAIAPIKey: nil,
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)
        let previous = PersistedSettingsSnapshot(
            preferences: PersistedPreferenceSnapshot(
                serverMode: ServerModeKind.attached.rawValue,
                attachedOrigin: ConfigurationStore.defaultAttachedOrigin,
                bundledPort: ConfigurationStore.defaultBundledPort,
                developmentBinaryOverride: try bundledExecutableFixture(),
                rustLogLevel: "phoenix_ide=info"
            ),
            secrets: [
                .anthropicAPIKey: .unloaded,
                .openAIAPIKey: .unloaded,
            ]
        )
        var draft = previous.draft()
        draft.mode = .bundled
        draft.bundledPort = 8420
        draft.developmentBinaryOverride = try bundledExecutableFixture()
        draft.anthropicKey = ""
        draft.openAIKey = "new-openai"

        let result = try persistence.persist(draft: draft, appliedSnapshot: previous)

        XCTAssertEqual(try keychain.read(.anthropicAPIKey), "preserved-secret")
        XCTAssertEqual(try keychain.read(.openAIAPIKey), "new-openai")
        XCTAssertEqual(result.persistedSnapshot.secrets[.anthropicAPIKey], .unloaded)
        XCTAssertEqual(result.persistedSnapshot.secrets[.openAIAPIKey], .loaded("new-openai"))
    }

    func testConfigurationStoreLoadRejectsMissingSavedMode() {
        let suite = "ConfigurationStore.load.missing-mode.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }

        XCTAssertThrowsError(try ConfigurationStore.load(bundle: .main, defaults: defaults)) { error in
            XCTAssertEqual(error as? ConfigurationError, .missingModeSelection)
        }
    }

    func testSettingsPersistenceLoadsUnselectedFirstRunWithoutPersistedMode() throws {
        let suite = "SettingsPersistence.load.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let persistence = SettingsPersistence(defaults: defaults, keychain: KeychainStore(service: "test.settings.load"), bundle: .main)

        let loaded = try persistence.loadDraft()
        XCTAssertFalse(loaded.hasSavedModeSelection)
        XCTAssertNil(loaded.draft.mode)
        XCTAssertEqual(loaded.draft.attachedOrigin, ConfigurationStore.defaultAttachedOrigin)
        XCTAssertEqual(loaded.draft.bundledPort, ConfigurationStore.defaultBundledPort)
    }

    func testSettingsPersistenceLoadDraftPropagatesSecretReadFailure() {
        let suite = "SettingsPersistence.load.failure.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = ScriptedSecretStore(scriptedReads: [
            .anthropicAPIKey: [.fail(.status(errSecInteractionNotAllowed))]
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)

        XCTAssertThrowsError(try persistence.loadDraft()) { error in
            XCTAssertEqual(error as? KeychainError, .status(errSecInteractionNotAllowed))
        }
    }

    func testSettingsPersistencePersistsOnlyOnApply() throws {
        let suite = "SettingsPersistence.persist.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let keychain = ScriptedSecretStore(values: [
            .anthropicAPIKey: "old-anthropic"
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)
        var draft = SettingsDraft.defaults
        draft.mode = .attached
        draft.attachedOrigin = "https://phoenix.example.test:8031"
        draft.rustLogLevel = "phoenix_ide=debug"
        draft.anthropicKey = "anthropic-secret"

        XCTAssertNil(defaults.object(forKey: PreferenceKey.serverMode))
        XCTAssertNil(defaults.object(forKey: PreferenceKey.attachedOrigin))

        let persisted = try persistence.persist(draft: draft)
        guard case .attached(let attached) = persisted.candidate else {
            return XCTFail("expected attached candidate")
        }
        XCTAssertEqual(attached.origin.description, "https://phoenix.example.test:8031")
        XCTAssertEqual(defaults.string(forKey: PreferenceKey.serverMode), ServerModeKind.attached.rawValue)
        XCTAssertEqual(defaults.string(forKey: PreferenceKey.attachedOrigin), "https://phoenix.example.test:8031")
        XCTAssertEqual(defaults.string(forKey: PreferenceKey.rustLogLevel), "phoenix_ide=debug")
        XCTAssertEqual(try keychain.read(.anthropicAPIKey), "old-anthropic")
        XCTAssertTrue(persisted.summary.savedSecrets.isEmpty)
        XCTAssertTrue(persisted.summary.deletedSecrets.isEmpty)
        XCTAssertTrue(persisted.summary.requiresReconnect)
    }

    func testSettingsPersistenceBundledApplyWritesConfiguredSecrets() throws {
        let suite = "SettingsPersistence.persist.bundled.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let service = "test.settings.persist.bundled.\(UUID().uuidString)"
        let keychain = KeychainStore(service: service)
        for secret in ProviderSecret.allCases { try? keychain.write("", for: secret) }
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)
        var draft = SettingsDraft.defaults
        draft.mode = .bundled
        draft.bundledPort = 8420
        draft.developmentBinaryOverride = try bundledExecutableFixture()
        draft.rustLogLevel = "phoenix_ide=debug"
        draft.anthropicKey = "anthropic-secret"

        let persisted = try persistence.persist(draft: draft)
        guard case .bundled(let bundled) = persisted.candidate else {
            return XCTFail("expected bundled candidate")
        }
        XCTAssertEqual(bundled.origin.description, "http://127.0.0.1:8420")
        XCTAssertEqual(defaults.string(forKey: PreferenceKey.serverMode), ServerModeKind.bundled.rawValue)
        XCTAssertEqual(defaults.object(forKey: PreferenceKey.bundledPort) as? Int, 8420)
        XCTAssertEqual(defaults.string(forKey: PreferenceKey.rustLogLevel), "phoenix_ide=debug")
        XCTAssertEqual(try keychain.read(.anthropicAPIKey), "anthropic-secret")
        XCTAssertEqual(persisted.summary.savedSecrets, [.anthropicAPIKey])
        XCTAssertTrue(persisted.summary.requiresReconnect)
        for secret in ProviderSecret.allCases { try? keychain.write("", for: secret) }
    }

    private func bundledExecutableFixture() throws -> String {
        "/bin/sh"
    }

    func testSettingsPersistenceRejectsApplyWithoutChoosingMode() {
        let suite = "SettingsPersistence.persist.missing-mode.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let persistence = SettingsPersistence(defaults: defaults, keychain: KeychainStore(service: "test.settings.missing.mode"), bundle: .main)

        XCTAssertThrowsError(try persistence.persist(draft: .defaults)) { error in
            XCTAssertEqual(error as? ConfigurationError, .missingModeSelection)
        }
    }

    func testSettingsPersistenceRollsBackPreferencesAndSecretsWhenSecondSecretWriteFails() throws {
        let suite = "SettingsPersistence.persist.rollback-second-write.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.bundled.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set("https://before.example.test", forKey: PreferenceKey.attachedOrigin)
        defaults.set(9420, forKey: PreferenceKey.bundledPort)
        defaults.set(try bundledExecutableFixture(), forKey: PreferenceKey.bundledDevelopmentBinary)
        defaults.set("phoenix_ide=info", forKey: PreferenceKey.rustLogLevel)
        let keychain = ScriptedSecretStore(values: [
            .anthropicAPIKey: "old-anthropic",
            .openAIAPIKey: "old-openai",
        ], scriptedWrites: [
            .succeed(.anthropicAPIKey),
            .fail(.openAIAPIKey, .status(errSecInteractionNotAllowed)),
            .succeed(.anthropicAPIKey),
            .succeed(.openAIAPIKey),
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)
        var draft = SettingsDraft.defaults
        draft.mode = .bundled
        draft.attachedOrigin = "https://after.example.test"
        draft.bundledPort = 9999
        draft.developmentBinaryOverride = try bundledExecutableFixture()
        draft.rustLogLevel = "phoenix_ide=debug"
        draft.anthropicKey = "new-anthropic"
        draft.openAIKey = "new-openai"

        XCTAssertThrowsError(try persistence.persist(draft: draft)) { error in
            guard let keychainError = error as? KeychainError else {
                return XCTFail("expected keychain error, got \(error)")
            }
            XCTAssertEqual(keychainError, .status(errSecInteractionNotAllowed))
        }

        XCTAssertEqual(defaults.string(forKey: PreferenceKey.serverMode), ServerModeKind.bundled.rawValue)
        XCTAssertEqual(defaults.string(forKey: PreferenceKey.attachedOrigin), "https://before.example.test")
        XCTAssertEqual(defaults.object(forKey: PreferenceKey.bundledPort) as? Int, 9420)
        XCTAssertEqual(defaults.string(forKey: PreferenceKey.bundledDevelopmentBinary), try bundledExecutableFixture())
        XCTAssertEqual(defaults.string(forKey: PreferenceKey.rustLogLevel), "phoenix_ide=info")
        XCTAssertEqual(try keychain.read(.anthropicAPIKey), "old-anthropic")
        XCTAssertEqual(try keychain.read(.openAIAPIKey), "old-openai")
    }

    func testSettingsPersistenceRollsBackDeleteThenWriteFailure() throws {
        let suite = "SettingsPersistence.persist.rollback-delete-write.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.bundled.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set(8420, forKey: PreferenceKey.bundledPort)
        defaults.set(try bundledExecutableFixture(), forKey: PreferenceKey.bundledDevelopmentBinary)
        let keychain = ScriptedSecretStore(values: [
            .anthropicAPIKey: "old-anthropic",
            .openAIAPIKey: "old-openai",
        ], scriptedWrites: [
            .succeed(.anthropicAPIKey),
            .fail(.openAIAPIKey, .status(errSecInteractionNotAllowed)),
            .succeed(.anthropicAPIKey),
            .succeed(.openAIAPIKey),
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)
        var draft = SettingsDraft.defaults
        draft.mode = .bundled
        draft.bundledPort = 8420
        draft.developmentBinaryOverride = try bundledExecutableFixture()
        draft.anthropicKey = ""
        draft.openAIKey = "new-openai"

        XCTAssertThrowsError(try persistence.persist(draft: draft))
        XCTAssertEqual(try keychain.read(.anthropicAPIKey), "old-anthropic")
        XCTAssertEqual(try keychain.read(.openAIAPIKey), "old-openai")
    }

    func testSettingsPersistenceRollbackRestoresEveryTouchedSecretAfterAttachedToBundledFailure() throws {
        let suite = "SettingsPersistence.persist.rollback-attached-to-bundled.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.attached.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set("https://before.example.test", forKey: PreferenceKey.attachedOrigin)
        let keychain = ScriptedSecretStore(values: [
            .anthropicAPIKey: "preserved-anthropic",
            .openAIAPIKey: nil,
        ], scriptedWrites: [
            .succeed(.openAIAPIKey),
            .fail(.anthropicAPIKey, .status(errSecInteractionNotAllowed)),
            .succeed(.anthropicAPIKey),
            .succeed(.openAIAPIKey),
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)
        let previous = PersistedSettingsSnapshot(
            preferences: PersistedPreferenceSnapshot(
                serverMode: ServerModeKind.attached.rawValue,
                attachedOrigin: "https://before.example.test",
                bundledPort: ConfigurationStore.defaultBundledPort,
                developmentBinaryOverride: "",
                rustLogLevel: "phoenix_ide=info"
            ),
            secrets: [
                .anthropicAPIKey: .preserveUnloaded,
                .openAIAPIKey: .unloaded,
            ]
        )
        var draft = SettingsDraft.defaults
        draft.mode = .bundled
        draft.attachedOrigin = "https://after.example.test"
        draft.bundledPort = 8420
        draft.developmentBinaryOverride = try bundledExecutableFixture()
        draft.anthropicKey = ""
        draft.openAIKey = "new-openai"

        XCTAssertThrowsError(try persistence.persist(draft: draft, appliedSnapshot: previous))
        XCTAssertEqual(try keychain.read(.anthropicAPIKey), "preserved-anthropic")
        XCTAssertNil(try keychain.read(.openAIAPIKey))
    }

    func testSettingsPersistenceDoesNotPerformFalliblePostWriteKeychainRead() throws {
        let suite = "SettingsPersistence.persist.no-post-read.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        let store = ScriptedSecretStore(scriptedReads: [
            .anthropicAPIKey: [.succeed(nil), .fail(.status(errSecInteractionNotAllowed))],
            .openAIAPIKey: [.succeed(nil), .fail(.status(errSecInteractionNotAllowed))],
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: store, bundle: .main)
        var draft = SettingsDraft.defaults
        draft.mode = .attached
        draft.attachedOrigin = "https://phoenix.example.test:8031"

        let result = try persistence.persist(draft: draft)
        XCTAssertEqual(result.persistedSnapshot.preferences.serverMode, ServerModeKind.attached.rawValue)
        XCTAssertEqual(result.persistedSnapshot.secrets[.anthropicAPIKey], .loaded(nil))
        XCTAssertEqual(result.persistedSnapshot.secrets[.openAIAPIKey], .loaded(nil))
    }

    func testSettingsPersistenceSurfacesRollbackFailureAsCompoundError() throws {
        let suite = "SettingsPersistence.persist.rollback-compound.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.bundled.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set("https://before.example.test", forKey: PreferenceKey.attachedOrigin)
        defaults.set(8420, forKey: PreferenceKey.bundledPort)
        defaults.set(try bundledExecutableFixture(), forKey: PreferenceKey.bundledDevelopmentBinary)
        let failingWriteStore = ScriptedSecretStore(values: [
            .anthropicAPIKey: "old-anthropic",
            .openAIAPIKey: "old-openai",
        ], scriptedWrites: [
            .succeed(.anthropicAPIKey),
            .fail(.openAIAPIKey, .status(errSecInteractionNotAllowed)),
            .fail(.anthropicAPIKey, .status(errSecInteractionNotAllowed)),
            .succeed(.openAIAPIKey),
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: failingWriteStore, bundle: .main)
        let snapshot = PersistedSettingsSnapshot(
            preferences: PersistedPreferenceSnapshot(
                serverMode: ServerModeKind.bundled.rawValue,
                attachedOrigin: "https://before.example.test",
                bundledPort: 8420,
                developmentBinaryOverride: try bundledExecutableFixture(),
                rustLogLevel: nil
            ),
            secrets: [
                .anthropicAPIKey: .loaded("old-anthropic"),
                .openAIAPIKey: .loaded("old-openai"),
            ]
        )
        var draft = SettingsDraft.defaults
        draft.mode = .bundled
        draft.attachedOrigin = "https://after.example.test"
        draft.bundledPort = 8420
        draft.developmentBinaryOverride = try bundledExecutableFixture()
        draft.anthropicKey = "new-anthropic"
        draft.openAIKey = "new-openai"

        XCTAssertThrowsError(try persistence.persist(draft: draft, appliedSnapshot: snapshot)) { error in
            guard case .applyFailedWithRollbackFailure(let cause, let rollbackFailure) = error as? SettingsPersistenceError else {
                return XCTFail("expected compound rollback failure, got \(error)")
            }
            XCTAssertEqual(cause as? KeychainError, .status(errSecInteractionNotAllowed))
            let compound = try? XCTUnwrap(rollbackFailure as? RollbackFailure)
            XCTAssertEqual(compound?.errors.count, 1)
            XCTAssertEqual(compound?.errors.first as? KeychainError, .status(errSecInteractionNotAllowed))
        }
    }

    func testSettingsPersistenceReconnectSummaryUsesExplicitPriorAppliedSnapshot() throws {
        let suite = "SettingsPersistence.persist.prior-applied-summary.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.attached.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set("https://stale.example.test", forKey: PreferenceKey.attachedOrigin)
        let keychain = ScriptedSecretStore(values: [
            .anthropicAPIKey: "current-secret"
        ])
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)
        let priorAppliedSnapshot = PersistedSettingsSnapshot(
            preferences: PersistedPreferenceSnapshot(
                serverMode: ServerModeKind.attached.rawValue,
                attachedOrigin: "https://current.example.test",
                bundledPort: ConfigurationStore.defaultBundledPort,
                developmentBinaryOverride: "",
                rustLogLevel: "phoenix_ide=info"
            ),
            secrets: [
                .anthropicAPIKey: .loaded("current-secret"),
                .openAIAPIKey: .unloaded,
            ]
        )
        var draft = priorAppliedSnapshot.draft()
        draft.mode = .attached

        let result = try persistence.persist(draft: draft, appliedSnapshot: priorAppliedSnapshot)

        XCTAssertFalse(result.summary.requiresReconnect)
    }

    func testAttachedPersistSkipsKeychainReadsAndWrites() throws {
        let suite = "SettingsPersistence.persist.attached-skips-keychain.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.attached.rawValue, forKey: PreferenceKey.serverMode)
        defaults.set("https://before.example.test", forKey: PreferenceKey.attachedOrigin)
        let keychain = ScriptedSecretStore(
            scriptedReads: [
                .anthropicAPIKey: [.fail(.status(errSecInteractionNotAllowed))],
                .openAIAPIKey: [.fail(.status(errSecInteractionNotAllowed))],
            ],
            scriptedWrites: [
                .fail(.anthropicAPIKey, .status(errSecInteractionNotAllowed))
            ]
        )
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)
        var draft = SettingsDraft.defaults
        draft.mode = .attached
        draft.attachedOrigin = "https://after.example.test"
        draft.anthropicKey = "should-not-touch-keychain"

        let result = try persistence.persist(draft: draft)

        XCTAssertEqual(result.candidate.kind, .attached)
        XCTAssertTrue(result.summary.savedSecrets.isEmpty)
        XCTAssertTrue(result.summary.deletedSecrets.isEmpty)
    }

    func testPersistedSnapshotSkipsKeychainWhenSavedModeIsAttached() throws {
        let suite = "SettingsPersistence.snapshot.attached-skips-keychain.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set(ServerModeKind.attached.rawValue, forKey: PreferenceKey.serverMode)
        let keychain = ScriptedSecretStore(
            scriptedReads: [
                .anthropicAPIKey: [.fail(.status(errSecInteractionNotAllowed))],
                .openAIAPIKey: [.fail(.status(errSecInteractionNotAllowed))],
            ]
        )
        let persistence = SettingsPersistence(defaults: defaults, keychain: keychain, bundle: .main)

        let snapshot = try persistence.persistedSnapshot()

        XCTAssertEqual(snapshot.secrets[.anthropicAPIKey], .unloaded)
        XCTAssertEqual(snapshot.secrets[.openAIAPIKey], .unloaded)
    }

    func testReopenDecisionShowsMainWindowOnlyWhenHidden() {
        XCTAssertTrue(ReopenDecision.shouldShowMainWindow(mainWindowIsVisible: false))
        XCTAssertFalse(ReopenDecision.shouldShowMainWindow(mainWindowIsVisible: true))
    }

    func testFirstRunDecisionOpensSettingsOnlyWithoutSavedMode() {
        XCTAssertTrue(FirstRunDecision.shouldOpenSettings(hasSavedModeSelection: false))
        XCTAssertFalse(FirstRunDecision.shouldOpenSettings(hasSavedModeSelection: true))
    }

    func testSidecarPackagingValidationRequiresExactIdentityName() {
        let mismatch = SidecarPackagingValidation.validatePackagedSidecar(
            helperExists: true,
            actualArchitectures: ["arm64"],
            requiredArchitectures: ["arm64"],
            actualSigningIdentity: "Developer ID Application: Other",
            expectedSigning: .identity("Developer ID Application: Phoenix")
        )
        XCTAssertTrue(mismatch.signingMismatch)
    }

    func testSidecarPackagingValidationRequiresHelperArchitecturesAndExpectedSigning() {
        let ok = SidecarPackagingValidation.validatePackagedSidecar(
            helperExists: true,
            actualArchitectures: ["arm64", "x86_64"],
            requiredArchitectures: ["arm64"],
            actualSigningIdentity: "Developer ID Application: Phoenix",
            expectedSigning: .identity("Developer ID Application: Phoenix")
        )
        XCTAssertTrue(ok.helperExists)
        XCTAssertEqual(ok.missingArchitectures, [])
        XCTAssertFalse(ok.signingMismatch)

        let bad = SidecarPackagingValidation.validatePackagedSidecar(
            helperExists: false,
            actualArchitectures: ["arm64"],
            requiredArchitectures: ["arm64", "x86_64"],
            actualSigningIdentity: nil,
            expectedSigning: .adHoc
        )
        XCTAssertFalse(bad.helperExists)
        XCTAssertEqual(bad.missingArchitectures, ["x86_64"])
        XCTAssertTrue(bad.signingMismatch)
    }

    func testBundledEnvironmentIsLoopbackTLSOffAndPrivate() throws {
        let root = URL(fileURLWithPath: "/tmp/Phoenix Tests")
        let configuration = BundledServerConfiguration(
            origin: try PhoenixOrigin("http://127.0.0.1:8420"),
            executableURL: root.appendingPathComponent("phoenix_ide"),
            runtimeRootURL: root,
            dataDirectoryURL: root.appendingPathComponent(".phoenix-ide"),
            databaseURL: root.appendingPathComponent(".phoenix-ide/phoenix.db"),
            phoenixLogURL: root.appendingPathComponent(".phoenix-ide/phoenix.log"),
            launcherLogURL: root.appendingPathComponent(".phoenix-ide/launcher.log"),
            ownerLockURL: root.appendingPathComponent("owner.lock"),
            rustLogLevel: "phoenix_ide=debug"
        )
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_BIND_ADDR"], "127.0.0.1")
        XCTAssertEqual(configuration.publicEnvironment["HOME"], "/tmp/Phoenix Tests")
        XCTAssertEqual(configuration.publicEnvironment["CODEX_HOME"], "/tmp/Phoenix Tests/.codex")
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_DATA_DIR"], "/tmp/Phoenix Tests/.phoenix-ide")
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_TLS"], "off")
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_PORT"], "8420")
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_DB_PATH"], "/tmp/Phoenix Tests/.phoenix-ide/phoenix.db")
        XCTAssertNil(configuration.publicEnvironment["ANTHROPIC_API_KEY"])
        XCTAssertNil(configuration.publicEnvironment["PHOENIX_PASSWORD"])
    }

    func testBundledPathsKeepOwnerLockOutsidePhoenixDataDir() throws {
        let bundle = Bundle.main
        let candidate = try ConfigurationStore.loadCandidate(
            kind: .bundled,
            attachedOrigin: ConfigurationStore.defaultAttachedOrigin,
            bundledPort: 8420,
            developmentBinaryOverride: "/bin/sh",
            rustLogLevel: "phoenix_ide=info",
            bundle: bundle
        )
        guard case .bundled(let configuration) = candidate else {
            return XCTFail("Expected bundled configuration")
        }
        XCTAssertEqual(configuration.ownerLockURL.deletingLastPathComponent(), configuration.runtimeRootURL)
        XCTAssertEqual(configuration.dataDirectoryURL.deletingLastPathComponent(), configuration.runtimeRootURL)
        XCTAssertEqual(configuration.launcherLogURL.lastPathComponent, "launcher.log")
        XCTAssertEqual(configuration.phoenixLogURL.lastPathComponent, "phoenix.log")
    }

    func testOwnershipDecoderFailsClosedForFutureKind() throws {
        let future = Data(#"{"kind":"future_manager"}"#.utf8)
        let decoded = try JSONDecoder().decode(InstallationOwnership.self, from: future)
        XCTAssertEqual(decoded, .unknown("future_manager"))
        XCTAssertFalse(decoded.grantsManagedAuthority)
        XCTAssertTrue(InstallationOwnership.launchdManaged.grantsManagedAuthority)
        XCTAssertFalse(InstallationOwnership.development.grantsManagedAuthority)
    }

    func testLegacyPlaintextSecretIsDeletedNotMigrated() {
        let suite = "ConfigurationTests.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite)!
        defer { defaults.removePersistentDomain(forName: suite) }
        defaults.set("secret", forKey: PreferenceKey.legacyAnthropicAPIKey)
        ConfigurationStore.removeLegacyPlaintextSecret(defaults: defaults)
        XCTAssertNil(defaults.object(forKey: PreferenceKey.legacyAnthropicAPIKey))
    }

    func testDeepLinkValidationErrorsExplainWhyNavigationStayedPut() throws {
        let id = try XCTUnwrap(UUID(uuidString: "66a063b4-6a90-49f5-8edc-9ffa67cffcf6"))
        XCTAssertEqual(
            DeepLinkValidationError.conversationMissing(id).localizedDescription,
            "Phoenix could not find conversation 66a063b4-6a90-49f5-8edc-9ffa67cffcf6 at the configured origin."
        )
        XCTAssertEqual(
            DeepLinkValidationError.invalidHTTPStatus(500).localizedDescription,
            "Conversation deep link validation failed with HTTP status 500."
        )
    }

    func testPhoenixURLActionsOpenExistingConversationByUUIDOnly() throws {
        let id = try XCTUnwrap(UUID(uuidString: "66a063b4-6a90-49f5-8edc-9ffa67cffcf6"))
        XCTAssertEqual(
            PhoenixURLAction(url: URL(string: "phoenix://conversation/\(id.uuidString)")!),
            .conversation(id: id)
        )
        XCTAssertEqual(PhoenixURLAction(url: URL(string: "phoenix://status")!), .status)
        XCTAssertEqual(PhoenixURLAction(url: URL(string: "phoenix://open")!), .open)
        XCTAssertNil(PhoenixURLAction(url: URL(string: "phoenix://conversation/not-a-uuid")!))
        XCTAssertNil(PhoenixURLAction(url: URL(string: "phoenix://conversation/\(id)/extra")!))
        XCTAssertNil(PhoenixURLAction(url: URL(string: "phoenix://new?prompt=unsupported")!))
        XCTAssertNil(PhoenixURLAction(url: URL(string: "pa://conversation/\(id)")!))
    }

    func testAttachedAndBundledModesExposeOnlyTheirOwnConfiguration() throws {
        let attached = ServerMode.attached(AttachedServerConfiguration(origin: try PhoenixOrigin("https://host.test")))
        XCTAssertEqual(attached.kind, .attached)
        XCTAssertEqual(attached.origin.description, "https://host.test")

        let root = URL(fileURLWithPath: "/tmp/phoenix")
        let bundledConfig = BundledServerConfiguration(
            origin: try PhoenixOrigin("http://127.0.0.1:8420"),
            executableURL: root.appendingPathComponent("phoenix_ide"),
            runtimeRootURL: root,
            dataDirectoryURL: root.appendingPathComponent(".phoenix-ide"),
            databaseURL: root.appendingPathComponent(".phoenix-ide/phoenix.db"),
            phoenixLogURL: root.appendingPathComponent(".phoenix-ide/phoenix.log"),
            launcherLogURL: root.appendingPathComponent(".phoenix-ide/launcher.log"),
            ownerLockURL: root.appendingPathComponent("owner.lock"),
            rustLogLevel: "info"
        )
        let bundled = ServerMode.bundled(bundledConfig)
        XCTAssertEqual(bundled.kind, .bundled)
        XCTAssertEqual(bundled.origin.description, "http://127.0.0.1:8420")
    }

    func testDeploymentInfoDecodesOptionalInstanceID() throws {
        let json = Data(#"{"build":{"version":"1.0","git_sha":"abc"},"network":{"bind_address":"127.0.0.1:8420","socket_activated":false,"tls":{"enabled":false,"mode":null}},"instance_id":"123e4567-e89b-12d3-a456-426614174000","local_access":true,"installation_ownership":{"kind":"development"}}"#.utf8)
        let deployment = try JSONDecoder().decode(DeploymentInfo.self, from: json)
        XCTAssertEqual(deployment.instanceID, "123e4567-e89b-12d3-a456-426614174000")
    }
}

@MainActor
final class ServerManagerHelpersTests: XCTestCase {
    func testIdentityHTTPStatusClassifiesWrongEndpointSeparatelyFromOutage() {
        XCTAssertEqual(
            classifyServerIdentityError(IdentityHTTPStatusError(statusCode: 404)),
            .wrongService("Phoenix identity endpoint returned HTTP 404.")
        )
        XCTAssertEqual(
            classifyServerIdentityError(IdentityHTTPStatusError(statusCode: 405)),
            .wrongService("Phoenix identity endpoint returned HTTP 405.")
        )
        XCTAssertEqual(
            classifyServerIdentityError(IdentityHTTPStatusError(statusCode: 503)),
            .unavailable("Phoenix identity endpoint returned HTTP 503.")
        )
    }

    func testIdentityClassifierPreservesRedirectedErrors() {
        XCTAssertEqual(
            classifyServerIdentityError(ServerIdentityError.redirected("redirected elsewhere")),
            .redirected("redirected elsewhere")
        )
    }

    func testBundledReconnectQueueSchedulesOneStopAndUsesLatestCandidate() throws {
        let first = ServerMode.attached(AttachedServerConfiguration(origin: try PhoenixOrigin("https://first.example.test")))
        let latest = ServerMode.attached(AttachedServerConfiguration(origin: try PhoenixOrigin("https://latest.example.test")))
        var queue = BundledReconnectQueue()

        XCTAssertTrue(queue.request(first))
        XCTAssertFalse(queue.request(latest))
        XCTAssertEqual(queue.takeAfterStop(), latest)
        XCTAssertFalse(queue.stopScheduled)
        XCTAssertNil(queue.latestCandidate)
    }

    func testRollingLogWriterKeepsRecentTailWithinBound() throws {
        let directory = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("launcher.log")
        let writer = RollingLogWriter(url: url, maxBytes: 32)
        let handle = try writer.openForAppend()
        try handle.close()

        let reopened = try XCTUnwrap(writer.append(Data("line-one\nline-two\nline-three\nline-four\n".utf8)))
        defer { try? reopened.close() }

        let logged = try String(contentsOf: url, encoding: .utf8)
        XCTAssertEqual(logged, "line-two\nline-three\nline-four\n")
        let size = try XCTUnwrap((try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.intValue)
        XCTAssertLessThanOrEqual(size, 32)
    }

    func testRollingLogWriterReopenPreservesExistingBoundedTail() throws {
        let directory = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("launcher.log")
        try Data("old-one\nold-two\nold-three\n".utf8).write(to: url)
        let writer = RollingLogWriter(url: url, maxBytes: 20)

        let handle = try writer.openForAppend()
        try handle.close()

        XCTAssertEqual(try String(contentsOf: url, encoding: .utf8), "old-two\nold-three\n")
    }

    func testRollingLogWriterReadsOnlyBoundedTailOfHugeExistingFile() throws {
        let directory = URL(fileURLWithPath: NSTemporaryDirectory()).appendingPathComponent(UUID().uuidString, isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("launcher.log")
        let prefix = Data(repeating: 0x41, count: 1_000_000)
        var contents = prefix
        contents.append(Data("\nlast-one\nlast-two\n".utf8))
        try contents.write(to: url)
        let writer = RollingLogWriter(url: url, maxBytes: 24)

        let handle = try writer.openForAppend()
        try handle.close()

        XCTAssertEqual(try String(contentsOf: url, encoding: .utf8), "last-one\nlast-two\n")
        XCTAssertLessThanOrEqual((try Data(contentsOf: url)).count, 24)
    }

    func testRollingLogWriterPreservesWholeRedactedLineWhenItFitsBound() {
        let data = Data(("prefix\n[REDACTED sensitive log line]\nnext\n").utf8)
        let bounded = RollingLogWriter.boundedTail(data, maxBytes: 64)
        let logged = String(decoding: bounded, as: UTF8.self)

        XCTAssertEqual(logged, "prefix\n[REDACTED sensitive log line]\nnext\n")
        XCTAssertFalse(logged.hasPrefix("\n"))
    }

    func testRollingLogWriterHardBoundsOversizedNewlineDelimitedRecord() {
        let oversized = String(repeating: "A", count: 80)
        let data = Data(("keep\n\(oversized)\nnext\n").utf8)

        let bounded = RollingLogWriter.boundedTail(data, maxBytes: 32)
        let logged = String(decoding: bounded, as: UTF8.self)

        XCTAssertEqual(logged, "next\n")
    }

    func testRollingLogWriterHardBoundsOversizedNewlineFreeRecord() {
        let oversized = Data(String(repeating: "B", count: 90).utf8)
        let bounded = RollingLogWriter.boundedTail(oversized, maxBytes: 24)

        XCTAssertEqual(String(decoding: bounded, as: UTF8.self), String(repeating: "B", count: 24))
    }

    func testCertificateClassifierCoversFullCertificateFamily() {
        let codes: [URLError.Code] = [
            .secureConnectionFailed,
            .serverCertificateHasBadDate,
            .serverCertificateUntrusted,
            .serverCertificateHasUnknownRoot,
            .serverCertificateNotYetValid,
            .clientCertificateRejected,
            .clientCertificateRequired,
        ]
        for code in codes {
            XCTAssertTrue(isCertificateURLError(code), "Expected \(code) to be classified as certificate related")
            let classified = classifyServerIdentityError(URLError(code))
            guard case .tls = classified else {
                return XCTFail("Expected tls classification for \(code), got \(classified)")
            }
        }
        XCTAssertFalse(isCertificateURLError(.timedOut))
    }

    func testLogBufferBuffersPartialLinesUntilNewlineAndFlushesOnExit() {
        var buffer = ConnectionLogBuffer(maxLines: 4)
        let redact: (String) -> String = { line in line.replacingOccurrences(of: "secret=abc", with: "secret= [REDACTED]") }

        XCTAssertEqual(buffer.append(Data("first\nsecret=abc".utf8), redact: redact), ["first"])
        XCTAssertEqual(buffer.completeLines, ["first"])
        XCTAssertEqual(buffer.pendingFragment, "secret=abc")

        XCTAssertEqual(buffer.append(Data("\nsecond\nthird".utf8), redact: redact), ["secret= [REDACTED]", "second"])
        XCTAssertEqual(buffer.completeLines, ["first", "secret= [REDACTED]", "second"])
        XCTAssertEqual(buffer.flushPending(redact: redact), "third")
        XCTAssertEqual(buffer.completeLines, ["first", "secret= [REDACTED]", "second", "third"])
        XCTAssertNil(buffer.flushPending(redact: redact))
    }

    func testLogBufferRedactsSensitiveKeySplitAcrossChunks() {
        var buffer = ConnectionLogBuffer(maxLines: 4)
        let redact: (String) -> String = { line in
            let lowercased = line.lowercased()
            guard lowercased.contains("authorization") else { return line }
            if let separator = line.firstIndex(of: ":") {
                return String(line[...separator]) + " [REDACTED]"
            }
            return "[REDACTED sensitive log line]"
        }

        XCTAssertEqual(buffer.append(Data("Authoriz".utf8), redact: redact), [])
        XCTAssertEqual(buffer.pendingFragment, "Authoriz")

        XCTAssertEqual(buffer.append(Data("ation: bearer abc123\n".utf8), redact: redact), ["Authorization: [REDACTED]"])
        XCTAssertEqual(buffer.completeLines, ["Authorization: [REDACTED]"])
        XCTAssertEqual(buffer.pendingFragment, "")
    }

    func testLogBufferRetainsSplitMultibyteUTF8ScalarUntilFramed() {
        var buffer = ConnectionLogBuffer(maxLines: 4)
        let redact: (String) -> String = { $0 }
        let emoji = "🙂"
        let emojiBytes = Array(emoji.utf8)

        XCTAssertEqual(buffer.append(Data([emojiBytes[0], emojiBytes[1]]), redact: redact), [])
        XCTAssertEqual(buffer.pendingFragment, "�")

        XCTAssertEqual(buffer.append(Data([emojiBytes[2], emojiBytes[3], 0x0A]), redact: redact), [emoji])
        XCTAssertEqual(buffer.completeLines, [emoji])
        XCTAssertEqual(buffer.pendingFragment, "")
    }

    func testLogBufferHardBoundsOversizedAndNewlineFreeRecords() {
        var buffer = ConnectionLogBuffer(maxLines: 4, maxRecordBytes: 8)
        let secret = Data("ANTHROPIC_API_KEY=super-secret-without-newline".utf8)

        XCTAssertEqual(buffer.append(secret, redact: { $0 }), [])
        XCTAssertLessThanOrEqual(buffer.pendingBytes.count, 8)
        XCTAssertTrue(buffer.discardingOversizedRecord)
        XCTAssertEqual(buffer.flushPending(redact: { $0 }), ConnectionLogBuffer.oversizedRecordMarker)
        XCTAssertFalse(buffer.completeLines.joined().contains("super-secret"))

        let emitted = buffer.append(Data("123456789-too-long\nnormal\n".utf8), redact: { $0 })
        XCTAssertEqual(emitted, [ConnectionLogBuffer.oversizedRecordMarker, "normal"])
    }

    func testLogBufferPreservesNonUTF8BytesLossilyWithoutDroppingLine() {
        var buffer = ConnectionLogBuffer(maxLines: 4)
        let redact: (String) -> String = { $0 }
        let emitted = buffer.append(Data([0x66, 0x6F, 0x80, 0x6F, 0x0A]), redact: redact)

        XCTAssertEqual(emitted, ["fo�o"])
        XCTAssertEqual(buffer.completeLines, ["fo�o"])
        XCTAssertEqual(buffer.pendingFragment, "")
    }

    func testConnectionStateFailureVersionSurvivesPreservedStop() {
        let version = VersionInfo(version: "1.2.3", gitSHA: "abc123")
        let failure = FailureState(version: version, message: "Bundled Phoenix did not become ready. Open Connection Status to locate the app-owned log.")
        let state = ConnectionState.failed(failure)

        XCTAssertEqual(state.versionInfo, version)
        XCTAssertEqual(state.failureViewModel?.message, failure.message)
    }

    func testLegacyPlaintextSecretDeletesLegacyPreferenceDomain() {
        let currentSuite = "ConfigurationTests.current.\(UUID().uuidString)"
        let legacySuite = "com.scottopell.pa"
        let current = UserDefaults(suiteName: currentSuite)!
        let legacy = UserDefaults(suiteName: legacySuite)!
        defer {
            current.removePersistentDomain(forName: currentSuite)
            legacy.removeObject(forKey: PreferenceKey.legacyAnthropicAPIKey)
            legacy.synchronize()
        }

        current.set("current-secret", forKey: PreferenceKey.legacyAnthropicAPIKey)
        legacy.set("legacy-secret", forKey: PreferenceKey.legacyAnthropicAPIKey)
        ConfigurationStore.removeLegacyPlaintextSecret(defaults: current)
        XCTAssertNil(current.object(forKey: PreferenceKey.legacyAnthropicAPIKey))
        XCTAssertNil(legacy.object(forKey: PreferenceKey.legacyAnthropicAPIKey))
    }

    func testBundledDeploymentMustMatchExactLaunchInstance() {
        let instanceID = UUID()
        let matching = DeploymentInfo(
            build: BuildInfo(version: "1.2.3", gitSHA: "abc123"),
            network: NetworkInfo(bindAddress: "127.0.0.1:8420", socketActivated: false, tls: TLSInfo(enabled: false, mode: nil)),
            currentMode: nil,
            instanceID: instanceID.uuidString,
            localAccess: true,
            installationOwnership: .development
        )
        let mismatched = DeploymentInfo(
            build: matching.build,
            network: matching.network,
            currentMode: nil,
            instanceID: UUID().uuidString,
            localAccess: matching.localAccess,
            installationOwnership: matching.installationOwnership
        )

        XCTAssertTrue(deploymentMatchesBundledInstance(matching, instanceID: instanceID))
        XCTAssertFalse(deploymentMatchesBundledInstance(mismatched, instanceID: instanceID))
        XCTAssertFalse(deploymentMatchesBundledInstance(nil, instanceID: instanceID))
    }

    func testDeploymentViolationCanLeaveAttachedDeploymentViewableButReadOnly() throws {
        let version = VersionInfo(version: "1.2.3", gitSHA: "abc123")
        let deployment = DeploymentInfo(
            build: BuildInfo(version: "1.2.3", gitSHA: "abc123"),
            network: NetworkInfo(bindAddress: "127.0.0.1:8031", socketActivated: false, tls: TLSInfo(enabled: false, mode: nil)),
            currentMode: nil,
            instanceID: nil,
            localAccess: true,
            installationOwnership: .systemdManaged
        )
        let state = ConnectionState.unsupportedOwnership(version, deployment, "read-only")
        XCTAssertTrue(state.canDisplayWebView)
        XCTAssertEqual(state.versionInfo, version)
        XCTAssertEqual(state.deploymentInfo, deployment)
        XCTAssertEqual(state.failureViewModel, ConnectionErrorViewModel(message: "read-only", allowsReconnect: false))
    }

    func testConnectionReapplyUsesCurrentConfiguredWebOriginNotBindAddress() throws {
        let candidate = ServerMode.attached(AttachedServerConfiguration(origin: try PhoenixOrigin("https://phoenix.example.test:8031")))
        let version = VersionInfo(version: "1.2.3", gitSHA: "abc123")
        let deployment = DeploymentInfo(
            build: BuildInfo(version: version.version, gitSHA: version.gitSHA),
            network: NetworkInfo(bindAddress: "0.0.0.0:8031", socketActivated: false, tls: TLSInfo(enabled: true, mode: nil)),
            currentMode: CurrentModeInfo(serverMode: "attached", webOrigin: "https://phoenix.example.test:8031"),
            instanceID: nil,
            localAccess: false,
            installationOwnership: .launchdManaged
        )

        XCTAssertFalse(ConnectionReapplyDecision.evaluate(currentMode: candidate, currentState: .ready(version, deployment), candidate: candidate).requiresReconnect)
    }

    func testBundledReconnectQueueCanBeCancelled() throws {
        let candidate = ServerMode.attached(AttachedServerConfiguration(origin: try PhoenixOrigin("https://phoenix.example.test:8031")))
        var queue = BundledReconnectQueue()
        XCTAssertTrue(queue.request(candidate))

        queue.cancel()

        XCTAssertFalse(queue.stopScheduled)
        XCTAssertNil(queue.latestCandidate)
        XCTAssertNil(queue.takeAfterStop())
    }

    func testDeepLinkNavigationQueueWaitsForAuthenticatedPrimaryWebView() {
        XCTAssertFalse(DeepLinkNavigationDecision.shouldValidateQueuedConversation(
            pendingConversationID: UUID(),
            hasAuthenticatedPrimaryWebView: false,
            hasPrimaryWebView: true,
            hasConfiguredOrigin: true
        ))
        XCTAssertFalse(DeepLinkNavigationDecision.shouldValidateQueuedConversation(
            pendingConversationID: UUID(),
            hasAuthenticatedPrimaryWebView: true,
            hasPrimaryWebView: false,
            hasConfiguredOrigin: true
        ))
        XCTAssertFalse(DeepLinkNavigationDecision.shouldValidateQueuedConversation(
            pendingConversationID: UUID(),
            hasAuthenticatedPrimaryWebView: true,
            hasPrimaryWebView: true,
            hasConfiguredOrigin: false
        ))
        XCTAssertTrue(DeepLinkNavigationDecision.shouldValidateQueuedConversation(
            pendingConversationID: UUID(),
            hasAuthenticatedPrimaryWebView: true,
            hasPrimaryWebView: true,
            hasConfiguredOrigin: true
        ))
    }

    func testConnectionReapplySkipsOnlyHealthyMatchingConnection() throws {
        let candidate = ServerMode.attached(AttachedServerConfiguration(origin: try PhoenixOrigin("https://phoenix.example.test:8031")))
        let version = VersionInfo(version: "1.2.3", gitSHA: "abc123")
        let healthyDeployment = DeploymentInfo(
            build: BuildInfo(version: version.version, gitSHA: version.gitSHA),
            network: NetworkInfo(bindAddress: "phoenix.example.test:8031", socketActivated: false, tls: TLSInfo(enabled: true, mode: nil)),
            currentMode: nil,
            instanceID: nil,
            localAccess: false,
            installationOwnership: .launchdManaged
        )
        let wrongOriginDeployment = DeploymentInfo(
            build: healthyDeployment.build,
            network: NetworkInfo(bindAddress: "other.example.test:8031", socketActivated: false, tls: TLSInfo(enabled: true, mode: nil)),
            currentMode: nil,
            instanceID: nil,
            localAccess: false,
            installationOwnership: .launchdManaged
        )

        XCTAssertFalse(ConnectionReapplyDecision.evaluate(currentMode: candidate, currentState: .ready(version, healthyDeployment), candidate: candidate).requiresReconnect)
        XCTAssertTrue(ConnectionReapplyDecision.evaluate(currentMode: candidate, currentState: .ready(version, wrongOriginDeployment), candidate: candidate).requiresReconnect)
        XCTAssertTrue(ConnectionReapplyDecision.evaluate(currentMode: candidate, currentState: .unsupportedOwnership(version, healthyDeployment, "read-only"), candidate: candidate).requiresReconnect)
        XCTAssertTrue(ConnectionReapplyDecision.evaluate(currentMode: nil, currentState: .stopped, candidate: candidate).requiresReconnect)
    }

    func testSidecarLaunchEnvironmentUsesPrivateHomeButPassesSafeInheritedVars() {
        let environment = SidecarLaunchEnvironment.build(
            inherited: [
                "PATH": "/usr/bin",
                "TMPDIR": "/tmp/test",
                "LANG": "en_US.UTF-8",
                "HOME": "/Users/public",
                "SHELL": "/bin/zsh",
                "USER": "scott",
                "LOGNAME": "scott",
                "SSH_AUTH_SOCK": "/tmp/ssh.sock",
                "SECRET_TOKEN": "nope",
            ],
            privateHome: URL(fileURLWithPath: "/private/phoenix-home"),
            instanceID: UUID(uuidString: "123E4567-E89B-12D3-A456-426614174000")!,
            publicEnvironment: ["PHOENIX_DATA_DIR": "/private/phoenix-home/.phoenix-ide"],
            sidecarSecrets: ["ANTHROPIC_API_KEY": "secret"]
        )

        XCTAssertEqual(environment["HOME"], "/private/phoenix-home")
        XCTAssertEqual(environment["SHELL"], "/bin/zsh")
        XCTAssertEqual(environment["USER"], "scott")
        XCTAssertEqual(environment["LOGNAME"], "scott")
        XCTAssertEqual(environment["SSH_AUTH_SOCK"], "/tmp/ssh.sock")
        XCTAssertNil(environment["SECRET_TOKEN"])
        XCTAssertEqual(environment["ANTHROPIC_API_KEY"], "secret")
    }

    func testDeepLinkConversationValidationParsesConversationEnvelope() throws {
        let id = try XCTUnwrap(UUID(uuidString: "66a063b4-6a90-49f5-8edc-9ffa67cffcf6"))
        let body: [String: Any] = ["conversation": ["id": id.uuidString.lowercased()]]

        XCTAssertEqual(DeepLinkConversationValidation.extractConversationID(from: body), id)
        XCTAssertNil(DeepLinkConversationValidation.extractConversationID(from: ["id": id.uuidString.lowercased()]))
    }
}
