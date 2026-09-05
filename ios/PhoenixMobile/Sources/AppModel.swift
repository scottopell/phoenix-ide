import Foundation
import CryptoKit
import Observation
import UserNotifications

/// Root composition: server settings, connectivity, API client, stores, and
/// the active per-conversation sessions.

@MainActor
struct CoordinatorIdentityReceipt: Codable, Equatable, Sendable {
    let configurationIdentity: APIConfigurationIdentity
    let conversationId: String
}

@MainActor
protocol CoordinatorIdentityStore {
    func load(configurationIdentity: APIConfigurationIdentity) -> CoordinatorIdentityReceipt?
    func save(_ receipt: CoordinatorIdentityReceipt)
    func clear(configurationIdentity: APIConfigurationIdentity)
    func clearAll()
}

@MainActor
struct UserDefaultsCoordinatorIdentityStore: CoordinatorIdentityStore {
    private let defaultsKey = "phoenix.coordinatorIdentityReceipts"

    private func loadAll() -> [CoordinatorIdentityReceipt] {
        guard let data = UserDefaults.standard.data(forKey: defaultsKey) else { return [] }
        return (try? JSONDecoder().decode([CoordinatorIdentityReceipt].self, from: data)) ?? []
    }

    private func saveAll(_ receipts: [CoordinatorIdentityReceipt]) {
        guard let data = try? JSONEncoder().encode(receipts) else { return }
        UserDefaults.standard.set(data, forKey: defaultsKey)
    }

    func load(configurationIdentity: APIConfigurationIdentity) -> CoordinatorIdentityReceipt? {
        loadAll().first { $0.configurationIdentity == configurationIdentity }
    }

    func save(_ receipt: CoordinatorIdentityReceipt) {
        var receipts = loadAll().filter { $0.configurationIdentity != receipt.configurationIdentity }
        receipts.append(receipt)
        saveAll(receipts)
    }

    func clear(configurationIdentity: APIConfigurationIdentity) {
        saveAll(loadAll().filter { $0.configurationIdentity != configurationIdentity })
    }

    func clearAll() {
        UserDefaults.standard.removeObject(forKey: defaultsKey)
    }
}

enum HardDeleteFenceLoadResult: Equatable, Sendable {
    case accessible([PersistedHardDeleteFence])
    case inaccessible
}

struct PersistedHardDeleteFence: Codable, Equatable, Sendable {
    let persistenceScope: PersistenceScopeIdentity
    let aggregateAuthority: String
    let memberConversationIds: [String]

    var storageName: String {
        let identity = "\(persistenceScope.serverEndpoint)\u{1f}\(persistenceScope.credentialGeneration)\u{1f}\(aggregateAuthority)"
        let digest = SHA256.hash(data: Data(identity.utf8))
        return "hard-delete-" + digest.map { String(format: "%02x", $0) }.joined()
    }
}

enum HardDeleteFenceState: Sendable {
    case needsCommit
    case committed(PersistedHardDeleteFence)
}

struct HardDeleteCleanupContext: Sendable {
    let configurationEpoch: Int
    let configurationIdentity: APIConfigurationIdentity
    let aggregateAuthority: String
    let triggerConversationId: String?
    let memberConversationIds: Set<String>
    let fenceState: HardDeleteFenceState
}

@MainActor
protocol ConversationPersistenceStore {
    var listPersistenceContext: VersionedDiskContext? { get }
    var persistenceScope: PersistenceScopeIdentity? { get }
    func pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity) async -> Set<String>
    func persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity) -> Set<String>
    func hasCachedSnapshot(conversationId: String) -> Bool
    func hasAuthoritativeCachedSnapshot(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String
    ) -> Bool
    func inspectOutbox(conversationId: String) -> OutboxStoreInspection
    func outboxPersistence(
        conversationId: String,
        aggregateAuthority: String?,
        scope: PersistenceScopeIdentity
    ) -> OutboxPersistenceHandle
    func snapshotPersistence(conversationId: String) -> VersionedDiskWriter
    func persistedConversationIds(
        aggregateId: String,
        scope: PersistenceScopeIdentity
    ) -> Set<String>
    func persistedConversationIds(
        aggregateId: String,
        scope: PersistenceScopeIdentity,
        legacyScope: PersistenceScopeIdentity?
    ) -> Set<String>
    func resetConversationListCache() async
    func removePersistedConversationState(conversationId: String) async
    func removeAuthoritativePersistedConversationState(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String
    ) async -> Bool
    func removeAuthoritativePersistedConversationState(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String,
        legacyScope: PersistenceScopeIdentity?
    ) async -> Bool
    func removeAllPersistedConversationState() async
    func persistHardDeleteFence(_ fence: PersistedHardDeleteFence) async -> Bool
    func hardDeleteFences(persistenceScope: PersistenceScopeIdentity) -> HardDeleteFenceLoadResult
    func retireHardDeleteFence(_ fence: PersistedHardDeleteFence) async
}

extension ConversationPersistenceStore {
    func persistedConversationIds(
        aggregateId: String,
        scope: PersistenceScopeIdentity,
        legacyScope: PersistenceScopeIdentity?
    ) -> Set<String> {
        persistedConversationIds(aggregateId: aggregateId, scope: scope)
    }

    func removeAuthoritativePersistedConversationState(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String,
        legacyScope: PersistenceScopeIdentity?
    ) async -> Bool {
        await removeAuthoritativePersistedConversationState(
            conversationId: conversationId,
            configurationIdentity: configurationIdentity,
            aggregateAuthority: aggregateAuthority)
    }
}

@MainActor
struct DiskConversationPersistenceStore: ConversationPersistenceStore {
    let baseDirectory: URL
    let directory: URL
    private let context: VersionedDiskContext
    private let conversationListWriter: VersionedDiskWriter
    let persistenceScope: PersistenceScopeIdentity?
    var listPersistenceContext: VersionedDiskContext? { context }

    init(baseDirectory: URL? = nil, context: VersionedDiskContext? = nil) {
        let resolvedBaseDirectory = baseDirectory ?? DiskStore.baseDirectory
        self.baseDirectory = resolvedBaseDirectory
        self.directory = DiskStore.phoenixMobileDirectory(baseDirectory: resolvedBaseDirectory)
        self.context = context ?? DiskStore.versionedContext(baseDirectory: resolvedBaseDirectory)
        self.conversationListWriter = self.context.writer(name: "conversations", version: 2)
        self.persistenceScope = nil
    }

    func pendingOutboxOwnerTranscriptRowIds(scope: PersistenceScopeIdentity) async -> Set<String> {
        let schemaVersion = Outbox.schemaVersion
        let directory = directory
        return await Task.detached(priority: nil) {
            Set(DiskStore.names(in: directory, withPrefix: "outbox-").compactMap { name in
                guard name.hasPrefix("outbox-") else { return nil }
                let conversationId = String(name.dropFirst("outbox-".count))
                guard !conversationId.isEmpty else { return nil }
                let source = directory.appendingPathComponent(name).appendingPathExtension("json")
                switch DiskStore.loadVersionedResult(
                    PersistedOutboxEnvelope.self,
                    source: source,
                    version: schemaVersion)
                {
                case .missing:
                    return nil
                case .value(let envelope):
                    guard envelope.scope == scope else { return nil }
                    let hasVisiblePendingEntries = envelope.entries.contains {
                        $0.conversationId == conversationId &&
                        $0.isVisible &&
                        $0.status == .pending &&
                        !$0.acceptedByServer
                    }
                    return hasVisiblePendingEntries ? conversationId : nil
                case .incompatible, .unreadable:
                    return nil
                }
            })
        }.value
    }

    func persistedOutboxOwnerTranscriptRowIdsSnapshot(scope: PersistenceScopeIdentity) -> Set<String> {
        Set(DiskStore.names(in: directory, withPrefix: "outbox-").compactMap { name in
            guard name.hasPrefix("outbox-") else { return nil }
            let conversationId = String(name.dropFirst("outbox-".count))
            let source = directory.appendingPathComponent(name).appendingPathExtension("json")
            guard case .value(let envelope) = DiskStore.loadVersionedResult(
                PersistedOutboxEnvelope.self,
                source: source,
                version: Outbox.schemaVersion)
            else {
                return nil
            }
            guard envelope.scope == scope else { return nil }
            return envelope.entries.contains {
                $0.conversationId == conversationId &&
                $0.isVisible &&
                $0.status == .pending &&
                !$0.acceptedByServer
            } ? conversationId : nil
        })
    }

    func hasCachedSnapshot(conversationId: String) -> Bool {
        let source = directory.appendingPathComponent("conv-\(conversationId)").appendingPathExtension("json")
        guard case .value(let snapshot) = DiskStore.loadVersionedResult(
            ConversationSession.PersistedSnapshot.self,
            source: source,
            version: ConversationSession.snapshotSchemaVersion)
        else { return false }
        return snapshot.conversation != nil && snapshot.syncedAt != nil
    }

    func hasCachedSnapshot(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String,
        legacyScope: PersistenceScopeIdentity?
    ) -> Bool {
        let source = directory.appendingPathComponent("conv-\(conversationId)").appendingPathExtension("json")
        guard case .value(let snapshot) = DiskStore.loadVersionedResult(
            ConversationSession.PersistedSnapshot.self,
            source: source,
            version: ConversationSession.snapshotSchemaVersion),
            snapshot.conversation?.id == conversationId,
            snapshot.syncedAt != nil
        else { return false }
        guard snapshot.conversation?.aggregateIdentity == aggregateAuthority else { return false }
        if let authority = snapshot.authoritative {
            return authority.configurationIdentity.persistenceScope == configurationIdentity.persistenceScope
                && authority.aggregateAuthority == aggregateAuthority
        }
        return legacyScope == configurationIdentity.persistenceScope
    }

    func hasAuthoritativeCachedSnapshot(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String
    ) -> Bool {
        let source = directory.appendingPathComponent("conv-\(conversationId)").appendingPathExtension("json")
        guard case .value(let snapshot) = DiskStore.loadVersionedResult(
            ConversationSession.PersistedSnapshot.self,
            source: source,
            version: ConversationSession.snapshotSchemaVersion),
            snapshot.conversation?.id == conversationId,
            snapshot.conversation?.aggregateIdentity == aggregateAuthority,
            snapshot.syncedAt != nil,
            snapshot.authoritative?.configurationIdentity == configurationIdentity,
            snapshot.authoritative?.aggregateAuthority == aggregateAuthority
        else { return false }
        return true
    }

    func inspectOutbox(conversationId: String) -> OutboxStoreInspection {
        let source = directory.appendingPathComponent("outbox-\(conversationId)").appendingPathExtension("json")
        switch DiskStore.loadVersionedResult(
            PersistedOutboxEnvelope.self,
            source: source,
            version: Outbox.schemaVersion)
        {
        case .missing:
            return OutboxStoreInspection(conversationId: conversationId, state: .missing)
        case .value(let envelope):
            return OutboxStoreInspection(
                conversationId: conversationId,
                state: .accessible(
                    scope: envelope.scope,
                    aggregateAuthority: envelope.aggregateAuthority,
                    entries: envelope.entries))
        case .incompatible:
            return OutboxStoreInspection(conversationId: conversationId, state: .incompatibleNewerVersion)
        case .unreadable:
            return OutboxStoreInspection(conversationId: conversationId, state: .inaccessible)
        }
    }

    func outboxPersistence(
        conversationId: String,
        aggregateAuthority: String?,
        scope: PersistenceScopeIdentity
    ) -> OutboxPersistenceHandle {
        let source = directory.appendingPathComponent("outbox-\(conversationId)").appendingPathExtension("json")
        let writer = context.writer(destinationURL: source, version: Outbox.schemaVersion)
        return OutboxPersistenceHandle(
            inspect: { requestedConversationId in
                self.inspectOutbox(conversationId: requestedConversationId)
            },
            reserveRevision: { writer.reserveRevision() },
            save: { envelope, revision in await writer.save(envelope, revision: revision) },
            remove: { revision in await writer.remove(revision: revision) })
    }

    func snapshotPersistence(conversationId: String) -> VersionedDiskWriter {
        let destination = directory.appendingPathComponent("conv-\(conversationId)").appendingPathExtension("json")
        return context.writer(destinationURL: destination, version: ConversationSession.snapshotSchemaVersion)
    }

    func persistedConversationIds(
        aggregateId: String,
        scope: PersistenceScopeIdentity
    ) -> Set<String> {
        persistedConversationIds(aggregateId: aggregateId, scope: scope, legacyScope: nil)
    }

    func persistedConversationIds(
        aggregateId: String,
        scope: PersistenceScopeIdentity,
        legacyScope: PersistenceScopeIdentity?
    ) -> Set<String> {
        let snapshotIds = Set(DiskStore.names(in: directory, withPrefix: "conv-").compactMap { name -> String? in
            guard name.hasPrefix("conv-") else { return nil }
            let conversationId = String(name.dropFirst("conv-".count))
            let source = directory.appendingPathComponent(name).appendingPathExtension("json")
            let loaded: DiskStore.VersionedLoad<ConversationSession.PersistedSnapshot> = DiskStore.loadVersionedResult(
                ConversationSession.PersistedSnapshot.self,
                source: source,
                version: ConversationSession.snapshotSchemaVersion)
            guard !conversationId.isEmpty,
                  case .value(let snapshot) = loaded,
                  snapshot.conversation?.product_conversation_id == aggregateId,
                  snapshot.conversation?.id == conversationId,
                  snapshot.syncedAt != nil
            else { return nil }
            let currentAuthorityMatches = snapshot.authoritative?.configurationIdentity.persistenceScope == scope
                && snapshot.authoritative?.aggregateAuthority == aggregateId
            let provenLegacyMatches = snapshot.authoritative == nil
                && legacyScope == scope
            return currentAuthorityMatches || provenLegacyMatches ? conversationId : nil
        })
        let outboxIds = Set(DiskStore.names(in: directory, withPrefix: "outbox-").compactMap { name -> String? in
            guard name.hasPrefix("outbox-") else { return nil }
            let conversationId = String(name.dropFirst("outbox-".count))
            let source = directory.appendingPathComponent(name).appendingPathExtension("json")
            guard case .value(let envelope) = DiskStore.loadVersionedResult(
                PersistedOutboxEnvelope.self,
                source: source,
                version: Outbox.schemaVersion),
                envelope.scope == scope,
                envelope.aggregateAuthority == aggregateId,
                envelope.entries.contains(where: { $0.conversationId == conversationId && $0.isVisible })
            else { return nil }
            return conversationId
        })
        return snapshotIds.union(outboxIds)
    }

    func resetConversationListCache() async {
        let revision = conversationListWriter.reserveRevision()
        await conversationListWriter.remove(revision: revision)
    }

    func removePersistedConversationState(conversationId: String) async {
        let snapshotSource = directory.appendingPathComponent("conv-\(conversationId)").appendingPathExtension("json")
        let outboxSource = directory.appendingPathComponent("outbox-\(conversationId)").appendingPathExtension("json")
        let snapshotWriter = context.writer(destinationURL: snapshotSource, version: ConversationSession.snapshotSchemaVersion)
        let outboxWriter = context.writer(destinationURL: outboxSource, version: Outbox.schemaVersion)
        await snapshotWriter.remove(revision: snapshotWriter.reserveRevision())
        await outboxWriter.remove(revision: outboxWriter.reserveRevision())
    }

    func removeAuthoritativePersistedConversationState(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String
    ) async -> Bool {
        await removeAuthoritativePersistedConversationState(
            conversationId: conversationId,
            configurationIdentity: configurationIdentity,
            aggregateAuthority: aggregateAuthority,
            legacyScope: nil)
    }

    func removeAuthoritativePersistedConversationState(
        conversationId: String,
        configurationIdentity: APIConfigurationIdentity,
        aggregateAuthority: String,
        legacyScope: PersistenceScopeIdentity?
    ) async -> Bool {
        let snapshotSource = directory.appendingPathComponent("conv-\(conversationId)").appendingPathExtension("json")
        switch DiskStore.loadVersionedResult(
            ConversationSession.PersistedSnapshot.self,
            source: snapshotSource,
            version: ConversationSession.snapshotSchemaVersion)
        {
        case .missing:
            break
        case .value(let snapshot):
            let currentAuthorityMatches = snapshot.authoritative?.configurationIdentity.persistenceScope == configurationIdentity.persistenceScope
                && snapshot.authoritative?.aggregateAuthority == aggregateAuthority
            let provenLegacyMatches = snapshot.authoritative == nil
                && legacyScope == configurationIdentity.persistenceScope
                && snapshot.conversation?.aggregateIdentity == aggregateAuthority
            if snapshot.conversation?.id == conversationId,
               currentAuthorityMatches || provenLegacyMatches
            {
                let writer = context.writer(destinationURL: snapshotSource, version: ConversationSession.snapshotSchemaVersion)
                await writer.remove(revision: writer.reserveRevision())
            }
        case .incompatible, .unreadable:
            return false
        }

        let outboxSource = directory.appendingPathComponent("outbox-\(conversationId)").appendingPathExtension("json")
        switch DiskStore.loadVersionedResult(
            PersistedOutboxEnvelope.self,
            source: outboxSource,
            version: Outbox.schemaVersion)
        {
        case .missing:
            break
        case .value(let envelope):
            if envelope.scope == configurationIdentity.persistenceScope,
               envelope.aggregateAuthority == aggregateAuthority
            {
                let writer = context.writer(destinationURL: outboxSource, version: Outbox.schemaVersion)
                await writer.remove(revision: writer.reserveRevision())
            }
        case .incompatible, .unreadable:
            return false
        }

        let snapshotResolved: Bool
        switch DiskStore.loadVersionedResult(
            ConversationSession.PersistedSnapshot.self,
            source: snapshotSource,
            version: ConversationSession.snapshotSchemaVersion)
        {
        case .missing: snapshotResolved = true
        case .value(let snapshot):
            let currentAuthorityMatches = snapshot.authoritative?.configurationIdentity.persistenceScope == configurationIdentity.persistenceScope
                && snapshot.authoritative?.aggregateAuthority == aggregateAuthority
            let provenLegacyMatches = snapshot.authoritative == nil
                && legacyScope == configurationIdentity.persistenceScope
                && snapshot.conversation?.aggregateIdentity == aggregateAuthority
            snapshotResolved = !(currentAuthorityMatches || provenLegacyMatches)
        case .incompatible, .unreadable: snapshotResolved = false
        }
        let outboxResolved: Bool
        switch DiskStore.loadVersionedResult(
            PersistedOutboxEnvelope.self,
            source: outboxSource,
            version: Outbox.schemaVersion)
        {
        case .missing: outboxResolved = true
        case .value(let envelope):
            outboxResolved = envelope.scope != configurationIdentity.persistenceScope
                || envelope.aggregateAuthority != aggregateAuthority
        case .incompatible, .unreadable: outboxResolved = false
        }
        return snapshotResolved && outboxResolved
    }

    func persistHardDeleteFence(_ fence: PersistedHardDeleteFence) async -> Bool {
        let source = directory
            .appendingPathComponent(fence.storageName)
            .appendingPathExtension("json")
        let writer = context.writer(destinationURL: source, version: 1)
        return await writer.save(fence, revision: writer.reserveRevision())
    }

    func hardDeleteFences(persistenceScope: PersistenceScopeIdentity) -> HardDeleteFenceLoadResult {
        var fences: [PersistedHardDeleteFence] = []
        for name in DiskStore.names(in: directory, withPrefix: "hard-delete-") {
            let source = directory.appendingPathComponent(name).appendingPathExtension("json")
            switch DiskStore.loadVersionedResult(
                PersistedHardDeleteFence.self,
                source: source,
                version: 1)
            {
            case .missing:
                continue
            case .value(let fence):
                if fence.persistenceScope == persistenceScope {
                    fences.append(fence)
                }
            case .incompatible, .unreadable:
                return .inaccessible
            }
        }
        return .accessible(fences)
    }

    func retireHardDeleteFence(_ fence: PersistedHardDeleteFence) async {
        let source = directory.appendingPathComponent(fence.storageName).appendingPathExtension("json")
        let writer = context.writer(destinationURL: source, version: 1)
        await writer.remove(revision: writer.reserveRevision())
    }

    func removeAllPersistedConversationState() async {
        await context.removeAllAndWait()
    }
}

protocol CredentialStore {
    func loadLegacyPassword(account: String) -> String?
    func loadRecord(account: String) -> AppModel.CredentialRecord?
    func saveRecord(_ record: AppModel.CredentialRecord, account: String) throws
    func deleteRecord(account: String)
}

extension CredentialStore {
    func loadLegacyPassword(account: String) -> String? { nil }
}

struct KeychainCredentialStore: CredentialStore {
    func loadLegacyPassword(account: String) -> String? {
        Keychain.password(account: account)
    }

    func loadRecord(account: String) -> AppModel.CredentialRecord? {
        guard let data = Keychain.data(account: account),
              case .value(let record) = DiskStore.loadVersionedResult(
                  AppModel.CredentialRecord.self,
                  fileData: data,
                  version: AppModel.credentialRecordVersion)
        else { return nil }
        return record
    }

    func saveRecord(_ record: AppModel.CredentialRecord, account: String) throws {
        try Keychain.setData(
            DiskStore.encodeVersioned(record, version: AppModel.credentialRecordVersion),
            account: account)
    }

    func deleteRecord(account: String) {
        Keychain.delete(account: account)
    }
}

@MainActor
@Observable
final class AppModel {
    // MARK: - Settings

    private static let serverURLKey = "phoenix.serverURL"
    private static let trustSelfSignedKey = "phoenix.trustSelfSigned"
    nonisolated fileprivate static let credentialRecordAccount = "server-credentials"
    nonisolated fileprivate static let legacyPasswordAccount = "server-password"
    /// Shared with NewConversationView's @AppStorage. Cleared on sign-out:
    /// the value is a server-local filesystem path and must not leak (or be
    /// sent) to a different server configured later.
    static let lastCwdKey = "phoenix.lastCwd"

    private var configurationMutationDepth = 0

    var serverURLString: String {
        didSet {
            UserDefaults.standard.set(serverURLString, forKey: Self.serverURLKey)
            rebuildAPIAfterConfigurationMutationIfNeeded()
        }
    }

    private(set) var password: String
    private(set) var credentialGeneration: String
    private let legacySnapshotPersistenceScope: PersistenceScopeIdentity?

    var configurationIdentity: APIConfigurationIdentity? {
        api?.configurationIdentity
    }

    var trustSelfSigned: Bool {
        didSet {
            UserDefaults.standard.set(trustSelfSigned, forKey: Self.trustSelfSignedKey)
            rebuildAPIAfterConfigurationMutationIfNeeded()
        }
    }

    var isConfigured: Bool {
        api != nil
    }

    // MARK: - Services

    let connectivity = ConnectivityMonitor()
    let listStore: ConversationListStore
    private(set) var api: PhoenixAPI?
    /// Invalidates responses started with earlier server credentials or URL.
    private var apiGeneration = 0

    /// Sessions for conversations the user has opened, kept alive so their
    /// outboxes continue draining while the user navigates elsewhere.
    private var sessions: [String: ConversationSession] = [:]
    /// Short-lived delivery owners for persisted outboxes whose conversation
    /// is not open. Retaining one per conversation serializes every trigger
    /// through the session's single drain task.
    private var drainSessions: [String: ConversationSession] = [:]
    private let hasCachedSnapshot: (String) -> Bool
    private let conversationPersistenceStore: ConversationPersistenceStore
    private let coordinatorIdentityStore: CoordinatorIdentityStore
    private let credentialStore: CredentialStore
    private var persistedOutboxHydrated = false
    private var startupDrainGeneration = 0
    private var startupHardDeleteRecoveryTask: Task<Void, Never>?
    private var lastCompletedDrainGeneration = 0
    private var persistedOutboxDrainTask: Task<Void, Never>?
    private var persistedOutboxDrainTaskGeneration: Int?
    private var persistedOutboxDrainAuthorityGeneration = 0
    private var hardDeleteCleanupGenerationByConversationId: [String: Int] = [:]
    private var hardDeleteCleanupWaiters: [Int: [CheckedContinuation<Void, Never>]] = [:]
    private var completedHardDeleteCleanupGenerations: Set<Int> = []
    private var hardDeletedConversationIds: Set<String> = []
    private var nextHardDeleteCleanupGeneration = 0

    static func randomCredentialGenerationForTestsAndDefaults() -> String {
        let bytes = (0..<16).map { _ in UInt8.random(in: .min ... .max) }
        return Data(bytes).map { String(format: "%02x", $0) }.joined()
    }

    nonisolated static func ephemeralCredentialGeneration() -> String {
        UUID().uuidString.lowercased()
    }

    struct CredentialRecord: Codable, Equatable, Sendable {
        let password: String
        let generation: String
    }

    nonisolated fileprivate static let credentialRecordVersion = 1

    private static func mintedCredentialGeneration() -> String {
        randomCredentialGenerationForTestsAndDefaults()
    }

    private static func legacyCredentialGeneration(serverURL: String, password: String) -> String {
        let scope = "\(PersistenceScopeIdentity(serverURL: serverURL, credentialGeneration: "").serverEndpoint)\u{1f}\(password)"
        return "legacy-" + SHA256.hash(data: Data(scope.utf8))
            .map { String(format: "%02x", $0) }
            .joined()
    }

    private static func loadCredentialRecord(from credentialStore: CredentialStore) -> CredentialRecord? {
        credentialStore.loadRecord(account: Self.credentialRecordAccount)
    }

    private static func saveCredentialRecord(_ record: CredentialRecord, to credentialStore: CredentialStore) throws {
        try credentialStore.saveRecord(record, account: Self.credentialRecordAccount)
    }

    init(
        hasCachedSnapshot: ((String) -> Bool)? = nil,
        conversationPersistenceStore: ConversationPersistenceStore? = nil,
        coordinatorIdentityStore: CoordinatorIdentityStore? = nil,
        credentialStore: CredentialStore = KeychainCredentialStore()
    ) {
        self.conversationPersistenceStore = conversationPersistenceStore ?? DiskConversationPersistenceStore()
        self.hasCachedSnapshot = hasCachedSnapshot ?? self.conversationPersistenceStore.hasCachedSnapshot(conversationId:)
        productConversationDetails = [:]
        self.coordinatorIdentityStore = coordinatorIdentityStore ?? UserDefaultsCoordinatorIdentityStore()
        self.credentialStore = credentialStore
        let listContext = self.conversationPersistenceStore.listPersistenceContext ?? DiskStore.versionedContext()
        listStore = ConversationListStore(hasCachedSnapshot: self.hasCachedSnapshot, context: listContext)
        let persistedServerURL = UserDefaults.standard.string(forKey: Self.serverURLKey) ?? ""
        serverURLString = persistedServerURL
        let credentialRecord = Self.loadCredentialRecord(from: credentialStore)
        let legacyPassword = credentialRecord == nil
            ? credentialStore.loadLegacyPassword(account: Self.legacyPasswordAccount)
            : nil
        password = credentialRecord?.password ?? legacyPassword ?? ""
        let loadedCredentialGeneration = credentialRecord?.generation
            ?? legacyPassword.map { Self.legacyCredentialGeneration(serverURL: persistedServerURL, password: $0) }
            ?? ""
        credentialGeneration = loadedCredentialGeneration
        legacySnapshotPersistenceScope = legacyPassword.map { _ in
            PersistenceScopeIdentity(
                serverURL: persistedServerURL,
                credentialGeneration: loadedCredentialGeneration)
        }
        trustSelfSigned = UserDefaults.standard.object(forKey: Self.trustSelfSignedKey) as? Bool ?? true
        attention = AttentionMonitor(
            currentConversations: listStore.conversations,
            transcriptToAggregate: listStore.transcriptToAggregate)
        rebuildAPI()
        _ = connectivity.addRestoreObserver { [weak self] in
            self?.scheduleDeliveryTrigger(.connectivityRestore)
            Task { await self?.refreshList() }
        }
        notificationRouter.model = self
        UNUserNotificationCenter.current().delegate = notificationRouter
        coordinatorConversationId = api.flatMap {
            self.coordinatorIdentityStore.load(configurationIdentity: $0.configurationIdentity)?.conversationId
        }
        finishStartupHydration()
    }

    private func finishStartupHydration() {
        guard startupHardDeleteRecoveryTask == nil, let api else { return }
        let identity = api.configurationIdentity
        let generation = apiGeneration
        let fences: [PersistedHardDeleteFence]
        switch conversationPersistenceStore.hardDeleteFences(persistenceScope: identity.persistenceScope) {
        case .accessible(let loaded):
            fences = loaded
        case .inaccessible:
            persistedOutboxHydrated = false
            return
        }
        hardDeletedConversationIds.formUnion(fences.flatMap(\.memberConversationIds))
        guard !fences.isEmpty else {
            persistedOutboxHydrated = true
            schedulePersistedOutboxDrain()
            return
        }
        startupHardDeleteRecoveryTask = Task { @MainActor [weak self] in
            guard let self else { return }
            defer {
                if self.apiGeneration == generation,
                   self.api?.configurationIdentity == identity {
                    self.startupHardDeleteRecoveryTask = nil
                }
            }
            for fence in fences {
                await self.completePersistedHardDeleteFence(fence)
            }
            guard !Task.isCancelled,
                  self.apiGeneration == generation,
                  self.api?.configurationIdentity == identity else { return }
            self.persistedOutboxHydrated = true
            self.schedulePersistedOutboxDrain()
        }
    }

    private enum DeliveryTrigger {
        case connectivityRestore
        case foreground
    }

    private func resumeAfterDeliveryTrigger(_ trigger: DeliveryTrigger) async {
        let classifiedOnThisTrigger = !persistedOutboxHydrated
        if classifiedOnThisTrigger {
            finishStartupHydration()
            await startupHardDeleteRecoveryTask?.value
        }
        guard persistedOutboxHydrated else { return }
        for session in sessions.values {
            switch trigger {
            case .connectivityRestore:
                session.resyncAfterConnectivityRestore()
            case .foreground:
                session.resyncAfterForeground()
            }
        }
        if !classifiedOnThisTrigger {
            schedulePersistedOutboxDrain()
        }
    }

    private func completePersistedHardDeleteFence(_ fence: PersistedHardDeleteFence) async {
        guard let identity = api?.configurationIdentity,
              identity.persistenceScope == fence.persistenceScope else { return }
        await runHardDeleteCleanup(.init(
            configurationEpoch: apiGeneration,
            configurationIdentity: identity,
            aggregateAuthority: fence.aggregateAuthority,
            triggerConversationId: nil,
            memberConversationIds: Set(fence.memberConversationIds),
            fenceState: .committed(fence)))
    }

    private func schedulePersistedOutboxDrain() {
        startupDrainGeneration &+= 1
        triggerPersistedOutboxDrainIfNeeded()
    }

    private func cancelPersistedOutboxDrainAuthority() {
        persistedOutboxDrainAuthorityGeneration &+= 1
        persistedOutboxDrainTask?.cancel()
        persistedOutboxDrainTask = nil
        persistedOutboxDrainTaskGeneration = nil
    }

    private func triggerPersistedOutboxDrainIfNeeded() {
        guard persistedOutboxHydrated,
              connectivity.isOnline,
              api != nil,
              persistedOutboxDrainTask == nil,
              lastCompletedDrainGeneration < startupDrainGeneration
        else { return }
        let generation = startupDrainGeneration
        let authorityGeneration = persistedOutboxDrainAuthorityGeneration
        let apiIdentity = api?.configurationIdentity
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.runPersistedOutboxDrain(
                generation: generation,
                authorityGeneration: authorityGeneration,
                apiIdentity: apiIdentity)
        }
        persistedOutboxDrainTaskGeneration = generation
        persistedOutboxDrainTask = task
    }

    private func runPersistedOutboxDrain(
        generation: Int,
        authorityGeneration: Int,
        apiIdentity: APIConfigurationIdentity?
    ) async {
        guard persistedOutboxDrainAuthorityGeneration == authorityGeneration,
              api?.configurationIdentity == apiIdentity
        else { return finishPersistedOutboxDrain(generation: generation, authorityGeneration: authorityGeneration) }
        let drainedConversationIds = Set(await drainPersistedOutboxes(
            authorityGeneration: authorityGeneration,
            apiIdentity: apiIdentity))
        guard persistedOutboxDrainAuthorityGeneration == authorityGeneration,
              api?.configurationIdentity == apiIdentity
        else { return finishPersistedOutboxDrain(generation: generation, authorityGeneration: authorityGeneration) }
        let joinedConversationIds = drainedConversationIds.union(Set(sessions.keys))
        for conversationId in joinedConversationIds {
            guard persistedOutboxDrainAuthorityGeneration == authorityGeneration,
                  api?.configurationIdentity == apiIdentity
            else { return finishPersistedOutboxDrain(generation: generation, authorityGeneration: authorityGeneration) }
            let session = drainSessions[conversationId] ?? sessions[conversationId]
            guard let session,
                  let drainGeneration = session.drainOutbox()
            else { continue }
            _ = await session.awaitDrainOutbox(generation: drainGeneration)
            guard persistedOutboxDrainAuthorityGeneration == authorityGeneration,
                  api?.configurationIdentity == apiIdentity
            else { return finishPersistedOutboxDrain(generation: generation, authorityGeneration: authorityGeneration) }
            _ = await session.outbox.flushPersistence()
        }
        guard persistedOutboxDrainAuthorityGeneration == authorityGeneration,
              api?.configurationIdentity == apiIdentity
        else { return finishPersistedOutboxDrain(generation: generation, authorityGeneration: authorityGeneration) }
        lastCompletedDrainGeneration = max(lastCompletedDrainGeneration, generation)
        finishPersistedOutboxDrain(generation: generation, authorityGeneration: authorityGeneration)
    }

    private func finishPersistedOutboxDrain(generation: Int, authorityGeneration: Int) {
        if persistedOutboxDrainAuthorityGeneration == authorityGeneration,
           persistedOutboxDrainTaskGeneration == generation
        {
            persistedOutboxDrainTask = nil
            persistedOutboxDrainTaskGeneration = nil
            triggerPersistedOutboxDrainIfNeeded()
        }
    }

    private func rebuildAPIAfterConfigurationMutationIfNeeded() {
        guard configurationMutationDepth == 0 else { return }
        rebuildAPI()
    }

    private func performAtomicConfigurationMutation(_ body: () throws -> Void) rethrows {
        configurationMutationDepth += 1
        defer {
            configurationMutationDepth -= 1
            if configurationMutationDepth == 0 {
                rebuildAPI()
            }
        }
        try body()
    }

    private func rebuildAPI() {

        cancelPersistedOutboxDrainAuthority()
        startupHardDeleteRecoveryTask?.cancel()
        startupHardDeleteRecoveryTask = nil
        persistedOutboxHydrated = false
        hardDeletedConversationIds.removeAll()
        apiGeneration += 1
        let configuredAPI: PhoenixAPI?
        if let url = URL(string: serverURLString), url.host != nil {
            configuredAPI = PhoenixAPI(
                baseURL: url,
                password: password.isEmpty ? nil : password,
                allowSelfSigned: trustSelfSigned,
                configurationIdentity: APIConfigurationIdentity(
                    serverURL: url.absoluteString,
                    credentialGeneration: credentialGeneration,
                    trustSelfSigned: trustSelfSigned))
        } else {
            configuredAPI = nil
        }
        let previousConfigurationIdentity = api?.configurationIdentity
        api = configuredAPI
        for session in sessions.values { session.invalidateConfiguration() }
        for session in drainSessions.values { session.invalidateConfiguration() }
        let cachedDetails = Array(productConversationDetails.values)
        productConversationDetails.removeAll()
        for detail in cachedDetails {
            detail.invalidateConfiguration()
        }
        if let previousConfigurationIdentity,
           configuredAPI?.configurationIdentity != previousConfigurationIdentity
        {
            coordinatorIdentityStore.clear(configurationIdentity: previousConfigurationIdentity)
        }
        coordinatorConversationId = configuredAPI.flatMap {
            coordinatorIdentityStore.load(configurationIdentity: $0.configurationIdentity)?.conversationId
        }
        guard let configuredAPI else { return }
        for session in sessions.values { session.replaceAPI(configuredAPI) }
        for session in drainSessions.values { session.replaceAPI(configuredAPI) }
        finishStartupHydration()
    }

    func configure(serverURL: String, password: String, trustSelfSigned: Bool) throws {
        let nextCredentialGeneration = Self.mintedCredentialGeneration()
        let record = CredentialRecord(password: password, generation: nextCredentialGeneration)
        try Self.saveCredentialRecord(record, to: credentialStore)
        performAtomicConfigurationMutation {
            self.password = record.password
            self.credentialGeneration = record.generation
            self.trustSelfSigned = trustSelfSigned
            self.serverURLString = serverURL
        }
    }

    func session(for conversationId: String, aggregateAuthority: String? = nil) -> ConversationSession? {
        guard let api else { return nil }
        guard !hardDeletedConversationIds.contains(conversationId) else { return nil }
        if let existing = sessions[conversationId] { return existing }
        let onConversationUpdate: (Conversation) -> Void = { [weak self] conversation in
            self?.handleSessionConversationUpdate(conversation, transcriptRowId: conversationId)
        }
        let onHardDeleted: @MainActor (ConversationSession.HardDeleteContext) async -> Void = { [weak self] context in
            await self?.handleHardDeleted(context)
        }
        let session: ConversationSession
        if let draining = drainSessions.removeValue(forKey: conversationId) {
            draining.adoptOpenOwnership(
                onConversationUpdate: onConversationUpdate,
                onHardDeleted: onHardDeleted)
            session = draining
        } else {
            session = ConversationSession(
                conversationId: conversationId,
                api: api,
                connectivity: connectivity,
                outboxPersistence: conversationPersistenceStore.outboxPersistence(
                    conversationId: conversationId,
                    aggregateAuthority: aggregateIdentity(forTranscriptRowId: conversationId),
                    scope: api.configurationIdentity.persistenceScope),
                snapshotPersistence: conversationPersistenceStore.snapshotPersistence(conversationId: conversationId),
                retryTiming: LiveSessionTiming(),
                staleCheckTiming: LiveSessionTiming(),
                deliveryTriggerAllowed: { [weak self] in
                    self?.persistedOutboxHydrated == true
                },
                legacySnapshotPersistenceScope: legacySnapshotPersistenceScope,
                aggregateAuthority: aggregateAuthority ?? aggregateIdentity(forTranscriptRowId: conversationId),
                onConversationUpdate: onConversationUpdate,
                onHardDeleted: onHardDeleted)
        }
        sessions[conversationId] = session
        return session
    }

    private func aggregateIdentity(forTranscriptRowId transcriptRowId: String) -> String? {
        if let aggregateId = listStore.aggregateId(forTranscriptRowId: transcriptRowId) {
            return aggregateId
        }
        let persistedAggregateIds = Set(listStore.transcriptToAggregate.values)
            .union(productConversationDetails.keys)
        guard let scope = api?.configurationIdentity.persistenceScope else { return nil }
        for aggregateId in persistedAggregateIds {
            if conversationPersistenceStore.persistedConversationIds(
                aggregateId: aggregateId,
                scope: scope).contains(transcriptRowId)
            {
                return aggregateId
            }
        }
        return nil
    }

    private func mergeAggregateProjection(
        existing: Conversation,
        liveUpdate: Conversation,
        aggregateIdentity: String
    ) -> Conversation {
        Conversation(
            id: existing.id == liveUpdate.id ? liveUpdate.id : existing.id,
            product_conversation_id: aggregateIdentity,
            slug: existing.slug,
            title: existing.title,
            model: liveUpdate.model,
            cwd: liveUpdate.cwd,
            created_at: liveUpdate.created_at,
            updated_at: liveUpdate.updated_at,
            message_count: liveUpdate.message_count,
            state: liveUpdate.state,
            state_updated_at: liveUpdate.state_updated_at,
            branch_name: liveUpdate.branch_name,
            task_title: existing.task_title,
            archived: existing.archived,
            project_name: liveUpdate.project_name,
            conv_mode_label: liveUpdate.conv_mode_label,
            presentation_mode: liveUpdate.presentation_mode,
            requires_action: liveUpdate.requires_action,
            transcript_generation: liveUpdate.transcript_generation,
            runtime_role: existing.runtime_role ?? liveUpdate.runtime_role)
    }

    private func handleSessionConversationUpdate(_ conversation: Conversation, transcriptRowId: String) {
        guard let aggregateIdentity = aggregateIdentity(forTranscriptRowId: transcriptRowId),
              let existing = listStore.conversations.first(where: { $0.aggregateIdentity == aggregateIdentity })
        else {
            listStore.upsert(conversation)
            return
        }
        listStore.upsert(
            mergeAggregateProjection(
                existing: existing,
                liveUpdate: conversation,
                aggregateIdentity: aggregateIdentity))
    }

    private func handleAggregateHardDeleted(
        aggregateId: String,
        transcriptRowId: String?,
        segmentTranscriptRowIds: Set<String>
    ) async {
        guard let api else { return }
        let persistedMembers = conversationPersistenceStore.persistedConversationIds(
            aggregateId: aggregateId,
            scope: api.configurationIdentity.persistenceScope,
            legacyScope: legacySnapshotPersistenceScope)
        let listMembers = Set(listStore.transcriptToAggregate.compactMap { id, aggregate in
            aggregate == aggregateId ? id : nil
        })
        await runHardDeleteCleanup(.init(
            configurationEpoch: apiGeneration,
            configurationIdentity: api.configurationIdentity,
            aggregateAuthority: aggregateId,
            triggerConversationId: transcriptRowId,
            memberConversationIds: persistedMembers
                .union(listMembers)
                .union(segmentTranscriptRowIds)
                .union(Set([transcriptRowId].compactMap { $0 })),
            fenceState: .needsCommit))
    }

    private func runHardDeleteCleanup(_ context: HardDeleteCleanupContext) async {
        func contextIsCurrent() -> Bool {
            apiGeneration == context.configurationEpoch
                && api?.configurationIdentity == context.configurationIdentity
        }
        let fence: PersistedHardDeleteFence
        switch context.fenceState {
        case .needsCommit:
            fence = PersistedHardDeleteFence(
                persistenceScope: context.configurationIdentity.persistenceScope,
                aggregateAuthority: context.aggregateAuthority,
                memberConversationIds: context.memberConversationIds.sorted())
            guard await conversationPersistenceStore.persistHardDeleteFence(fence) else {
                guard contextIsCurrent() else { return }
                let memberIds = Set(fence.memberConversationIds)
                _ = beginHardDeleteCleanup(conversationIds: memberIds)
                productConversationDetails[context.aggregateAuthority]?.invalidateHardDeleted()
                productConversationDetails.removeValue(forKey: context.aggregateAuthority)
                for id in memberIds {
                    sessions.removeValue(forKey: id)?.revokeForHardDelete()
                    drainSessions.removeValue(forKey: id)?.revokeForHardDelete()
                }
                _ = await listStore.removeAndPersist(aggregateId: context.aggregateAuthority)
                return
            }
            guard contextIsCurrent() else { return }
        case .committed(let persisted):
            fence = persisted
        }

        guard contextIsCurrent() else { return }

        let memberIds = Set(fence.memberConversationIds)
        let cleanupGeneration = beginHardDeleteCleanup(conversationIds: memberIds)
        productConversationDetails[context.aggregateAuthority]?.invalidateHardDeleted()
        productConversationDetails.removeValue(forKey: context.aggregateAuthority)
        for id in memberIds {
            sessions.removeValue(forKey: id)?.revokeForHardDelete()
            drainSessions.removeValue(forKey: id)?.revokeForHardDelete()
        }
        if pendingOpenConversationId == context.aggregateAuthority
            || pendingOpenConversationId.map(memberIds.contains) == true
        {
            pendingOpenConversationId = nil
        }

        var removedAll = true
        for id in memberIds {
            let removed = await conversationPersistenceStore.removeAuthoritativePersistedConversationState(
                conversationId: id,
                configurationIdentity: context.configurationIdentity,
                aggregateAuthority: context.aggregateAuthority,
                legacyScope: legacySnapshotPersistenceScope)
            guard contextIsCurrent() else { return }
            removedAll = removedAll && removed
        }
        guard removedAll,
              await listStore.removeAndPersist(aggregateId: context.aggregateAuthority),
              contextIsCurrent()
        else {
            completeHardDeleteCleanup(generation: cleanupGeneration, conversationIds: memberIds)
            return
        }
        await conversationPersistenceStore.retireHardDeleteFence(fence)
        guard contextIsCurrent() else { return }
        completeHardDeleteCleanup(generation: cleanupGeneration, conversationIds: memberIds)
        UNUserNotificationCenter.current().removeDeliveredNotifications(
            withIdentifiers: ["attention-\(context.aggregateAuthority)"])
        UNUserNotificationCenter.current().removePendingNotificationRequests(
            withIdentifiers: ["attention-\(context.aggregateAuthority)"])
    }

    private func clearPersistedState(for conversationId: String) async {
        await conversationPersistenceStore.removePersistedConversationState(conversationId: conversationId)
    }

    private func handleHardDeleted(_ report: ConversationSession.HardDeleteContext) async {
        guard let api,
              api.configurationIdentity == report.configurationIdentity
        else { return }
        let persistedMembers = conversationPersistenceStore.persistedConversationIds(
            aggregateId: report.aggregateAuthority,
            scope: report.configurationIdentity.persistenceScope,
            legacyScope: legacySnapshotPersistenceScope)
        let listMembers = Set(listStore.transcriptToAggregate.compactMap { id, aggregate in
            aggregate == report.aggregateAuthority ? id : nil
        })
        await runHardDeleteCleanup(.init(
            configurationEpoch: apiGeneration,
            configurationIdentity: report.configurationIdentity,
            aggregateAuthority: report.aggregateAuthority,
            triggerConversationId: report.conversationId,
            memberConversationIds: persistedMembers.union(listMembers).union([report.conversationId]),
            fenceState: .needsCommit))
    }

    func refreshList() async {
        guard let api else { return }
        attentionEvidenceGeneration &+= 1
        await listStore.refresh(api: api)
        if listStore.lastError == nil {
            // The user is looking at fresh data — nothing here should nudge
            // them later.
            attention.seed(
                with: listStore.conversations,
                transcriptToAggregate: listStore.transcriptToAggregate)
        }
    }

    // MARK: - Needs-attention nudges

    private var productConversationDetails: [String: ProductConversationDetailModel] = [:]
    let attention: AttentionMonitor
    private let notificationRouter = NotificationRouter()
    private static let nudgesEnabledKey = "phoenix.backgroundNudges"
    private var nudgePreferenceGeneration = 0
    private var attentionEvidenceGeneration = 0

    private(set) var backgroundNudgesEnabled =
        UserDefaults.standard.bool(forKey: AppModel.nudgesEnabledKey)
    private(set) var nudgeAuthorizationHint: String?
    /// Set by a notification tap; the list view navigates and clears it.
    var pendingOpenConversationId: String?

    func resolvedNavigationConversationId(
        aggregateId: String?,
        latestTranscriptRowId: String
    ) -> String {
        if connectivity.isOnline {
            return latestTranscriptRowId
        }
        guard let aggregateId else { return latestTranscriptRowId }
        return listStore.cachedNavigationTranscriptRowId(
            forAggregateId: aggregateId,
            latestTranscriptRowId: latestTranscriptRowId)
    }

    func navigationConversationId(for conversation: Conversation) -> String {
        resolvedNavigationConversationId(
            aggregateId: conversation.product_conversation_id,
            latestTranscriptRowId: conversation.transcriptRowIdentity)
    }

    func existingSession(for conversationId: String) -> ConversationSession? {
        if let existing = sessions[conversationId] { return existing }
        return drainSessions[conversationId]
    }

    func configureForTesting(serverURL: String, password: String = "", trustSelfSigned: Bool = true) {
        let record = CredentialRecord(password: password, generation: Self.mintedCredentialGeneration())
        try? Self.saveCredentialRecord(record, to: credentialStore)
        performAtomicConfigurationMutation {
            self.password = record.password
            self.credentialGeneration = record.generation
            self.trustSelfSigned = trustSelfSigned
            self.serverURLString = serverURL
        }
    }

    func replaceAPIForTesting(_ api: PhoenixAPI) {
        cancelPersistedOutboxDrainAuthority()
        self.api = api
        coordinatorConversationId = coordinatorIdentityStore.load(configurationIdentity: api.configurationIdentity)?.conversationId
        schedulePersistedOutboxDrain()
    }

    func triggerPersistedOutboxDrainIfNeededForTesting() {
        triggerPersistedOutboxDrainIfNeeded()
    }

    enum PersistedOutboxDrainAwaitResult: Equatable {
        case completed(Int)
        case noCurrentDrain
        case notReady
    }

    func triggerStartupHardDeleteRecoveryForTesting() {
        persistedOutboxHydrated = false
        finishStartupHydration()
    }

    func awaitStartupHardDeleteRecoveryForTesting() async {
        await startupHardDeleteRecoveryTask?.value
    }

    func currentPersistedOutboxDrainGenerationForTesting() -> Int? {
        persistedOutboxDrainTaskGeneration
    }

    func forceEvictSessionForTesting(_ conversationId: String) {
        sessions.removeValue(forKey: conversationId)?.stop()
        drainSessions.removeValue(forKey: conversationId)?.stop()
    }

    func awaitHardDeleteCleanupForTesting(conversationId: String) async {
        guard let generation = hardDeleteCleanupGenerationByConversationId[conversationId] else { return }
        if completedHardDeleteCleanupGenerations.contains(generation) { return }
        await withCheckedContinuation { continuation in
            if completedHardDeleteCleanupGenerations.contains(generation) {
                continuation.resume()
            } else {
                hardDeleteCleanupWaiters[generation, default: []].append(continuation)
            }
        }
    }

    private func beginHardDeleteCleanup(conversationIds: Set<String>) -> Int {
        nextHardDeleteCleanupGeneration &+= 1
        let generation = nextHardDeleteCleanupGeneration
        hardDeletedConversationIds.formUnion(conversationIds)
        for conversationId in conversationIds {
            hardDeleteCleanupGenerationByConversationId[conversationId] = generation
        }
        return generation
    }

    private func completeHardDeleteCleanup(generation: Int, conversationIds: Set<String>) {
        completedHardDeleteCleanupGenerations.insert(generation)
        for conversationId in conversationIds where hardDeleteCleanupGenerationByConversationId[conversationId] == generation {
            hardDeleteCleanupGenerationByConversationId.removeValue(forKey: conversationId)
        }
        let waiters = hardDeleteCleanupWaiters.removeValue(forKey: generation) ?? []
        waiters.forEach { $0.resume() }
    }

    func awaitPersistedOutboxDrainForTesting(generation: Int) async -> PersistedOutboxDrainAwaitResult {
        if lastCompletedDrainGeneration >= generation {
            return .completed(generation)
        }
        if !persistedOutboxHydrated || api == nil || !connectivity.isOnline {
            return .notReady
        }
        guard persistedOutboxDrainTaskGeneration == generation,
              let task = persistedOutboxDrainTask
        else { return .noCurrentDrain }
        await task.value
        return lastCompletedDrainGeneration >= generation ? .completed(generation) : .noCurrentDrain
    }

    func awaitCurrentPersistedOutboxDrainForTesting() async -> PersistedOutboxDrainAwaitResult {
        guard let generation = persistedOutboxDrainTaskGeneration else {
            if !persistedOutboxHydrated || api == nil || !connectivity.isOnline {
                return .notReady
            }
            return .noCurrentDrain
        }
        return await awaitPersistedOutboxDrainForTesting(generation: generation)
    }

    func forceAggregateNotFoundCleanupForTesting(
        aggregateId: String,
        transcriptRowId: String?,
        memberIds: Set<String>
    ) async {
        await handleAggregateHardDeleted(
            aggregateId: aggregateId,
            transcriptRowId: transcriptRowId,
            segmentTranscriptRowIds: memberIds)
    }

    func persistedOutboxContents(for conversationId: String) -> Outbox.StoredContents {
        let inspection = conversationPersistenceStore.inspectOutbox(conversationId: conversationId)
        switch inspection.state {
        case .accessible:
            return inspection.visibleEntries.isEmpty ? .empty : .hasVisibleEntries
        case .missing:
            return .empty
        case .inaccessible, .incompatibleNewerVersion:
            return .inaccessible
        }
    }

    func productConversationDetailModel(
        for aggregateId: String,
        initialTranscriptRowId: String? = nil
    ) -> ProductConversationDetailModel {
        if let existing = productConversationDetails[aggregateId] {
            if initialTranscriptRowId != nil {
                existing.primeInitialTranscriptRowId(initialTranscriptRowId)
            }
            return existing
        }
        guard let api else {
            fatalError("ProductConversationDetailModel requires configured API")
        }
        let created = ProductConversationDetailModel(
            aggregateId: aggregateId,
            initialTranscriptRowId: initialTranscriptRowId,
            api: api,
            connectivity: connectivity,
            sessionProvider: { [weak self] transcriptRowId, aggregateAuthority in
                self?.session(for: transcriptRowId, aggregateAuthority: aggregateAuthority)
            },
            existingSession: { [weak self] transcriptRowId in
                self?.existingSession(for: transcriptRowId)
            },
            persistedOutboxContents: { [weak self] transcriptRowId in
                self?.persistedOutboxContents(for: transcriptRowId) ?? .empty
            },
            hasCachedSnapshot: { [weak self] transcriptRowId in
                guard let self, let api = self.api else { return false }
                if self.conversationPersistenceStore.hasAuthoritativeCachedSnapshot(
                    conversationId: transcriptRowId,
                    configurationIdentity: api.configurationIdentity,
                    aggregateAuthority: aggregateId)
                {
                    return true
                }
                guard let store = self.conversationPersistenceStore as? DiskConversationPersistenceStore else {
                    return false
                }
                return store.hasCachedSnapshot(
                    conversationId: transcriptRowId,
                    configurationIdentity: api.configurationIdentity,
                    aggregateAuthority: aggregateId,
                    legacyScope: self.legacySnapshotPersistenceScope)
            },
            handleDefinitiveNotFound: { [weak self] transcriptRowId, segmentTranscriptRowIds in
                await self?.handleAggregateHardDeleted(
                    aggregateId: aggregateId,
                    transcriptRowId: transcriptRowId,
                    segmentTranscriptRowIds: segmentTranscriptRowIds)
            },
            onConfigurationInvalidated: { [weak self] detail in
                guard let self else { return }
                if self.productConversationDetails[aggregateId] === detail {
                    self.productConversationDetails.removeValue(forKey: aggregateId)
                }
            })
        productConversationDetails[aggregateId] = created
        return created
    }

    func setBackgroundNudges(_ enabled: Bool) async {
        nudgePreferenceGeneration &+= 1
        let generation = nudgePreferenceGeneration
        nudgeAuthorizationHint = nil
        if enabled {
            guard await AttentionMonitor.requestAuthorization() else {
                guard generation == nudgePreferenceGeneration else { return }
                nudgeAuthorizationHint =
                    "Notifications are off for Phoenix in iOS Settings — enable them there first."
                backgroundNudgesEnabled = false
                UserDefaults.standard.set(false, forKey: Self.nudgesEnabledKey)
                return
            }
        }
        guard generation == nudgePreferenceGeneration else { return }
        backgroundNudgesEnabled = enabled
        UserDefaults.standard.set(enabled, forKey: Self.nudgesEnabledKey)
        if enabled {
            BackgroundRefresh.scheduleNext()
        } else {
            BackgroundRefresh.cancelPending()
        }
    }

    /// One background-fetch cycle: fetch the list, notify on attention
    /// transitions, and opportunistically freshen the cached list so the
    /// next cold open is newer. Returns success for BGTask accounting.
    func runBackgroundAttentionCheck() async -> Bool {
        guard backgroundNudgesEnabled, let api else { return false }
        let startedGeneration = apiGeneration
        let startedNudgeGeneration = nudgePreferenceGeneration
        let startedEvidenceGeneration = attentionEvidenceGeneration
        let listToken = listStore.externalRefreshToken()
        guard let fresh = try? await api.listConversations() else { return false }
        guard !Task.isCancelled,
              backgroundNudgesEnabled,
              apiGeneration == startedGeneration
        else { return false }
        guard !Task.isCancelled,
              backgroundNudgesEnabled,
              apiGeneration == startedGeneration,
              listStore.canApplyExternal(startedAt: listToken)
        else { return false }
        guard listStore.applyExternal(fresh, startedAt: listToken) else { return false }
        let isCurrent: @MainActor () -> Bool = { [weak self] in
            guard let self else { return false }
            return self.backgroundNudgesEnabled
                && self.apiGeneration == startedGeneration
                && self.nudgePreferenceGeneration == startedNudgeGeneration
                && self.attentionEvidenceGeneration == startedEvidenceGeneration
        }
        await attention.refreshAndNotifyIfNeeded(
            from: fresh,
            transcriptToAggregate: listStore.transcriptToAggregate,
            isCurrent: isCurrent)
        return isCurrent()
    }

    // MARK: - Coordinator

    /// The fleet Coordinator's conversation id, remembered across launches
    /// so its cached transcript opens offline and its list row is badged.
    /// Per-server state — cleared on sign-out.
    private(set) var coordinatorConversationId: String?

    var coordinatorAvailableOffline: Bool {
        guard let id = coordinatorConversationId else { return false }
        return hasCachedSnapshot(id)
    }


    /// Resolve the Coordinator conversation to open. Online: get-or-create
    /// on the server (it's an ordinary conversation; everything downstream
    /// is the normal conversation surface). Offline: fall back to the
    /// remembered id so the cached transcript still opens — asking new
    /// questions then queues through the outbox like any conversation.
    func openCoordinator() async -> String? {
        if let api, connectivity.isOnline {
            let startedGeneration = apiGeneration
            do {
                let conversation = try await api.ensureCoordinator()
                guard apiGeneration == startedGeneration, connectivity.isOnline else {
                    return nil
                }
                coordinatorConversationId = conversation.id
                coordinatorIdentityStore.save(CoordinatorIdentityReceipt(
                    configurationIdentity: api.configurationIdentity,
                    conversationId: conversation.id))
                listStore.upsert(conversation)
                return conversation.id
            } catch {
                guard !Task.isCancelled, apiGeneration == startedGeneration else { return nil }
                if let apiError = error as? APIError,
                   apiError.isTransport,
                   let cached = coordinatorConversationId,
                   hasCachedSnapshot(cached) {
                    return cached
                }
                lastActionError = (error as? APIError)?.errorDescription
                    ?? error.localizedDescription
                return nil
            }
        }
        if let cached = coordinatorConversationId,
           hasCachedSnapshot(cached) {
            return cached
        }
        lastActionError = "Opening the Coordinator offline needs a cached conversation."
        return nil
    }

    /// Online-only archive. Returns false with `lastActionError` on failure.
    var lastActionError: String?

    private func authoritativeAggregateMemberIds(
        aggregateId: String,
        triggeringTranscriptRowId: String
    ) -> Set<String> {
        let detailMembers = productConversationDetails[aggregateId]?.aggregateMemberTranscriptRowIds ?? []
        let listMembers = Set(listStore.transcriptToAggregate.compactMap { transcriptRowId, mappedAggregateId in
            mappedAggregateId == aggregateId ? transcriptRowId : nil
        })
        guard let api else { return detailMembers.union(listMembers).union([triggeringTranscriptRowId]) }
        return detailMembers
            .union(listMembers)
            .union([triggeringTranscriptRowId])
            .union(conversationPersistenceStore.persistedConversationIds(
                aggregateId: aggregateId,
                scope: api.configurationIdentity.persistenceScope))
    }

    func closeUnavailableExplanation(for conversation: Conversation) -> String? {
        guard conversation.product_conversation_id != nil else { return nil }
        guard let snapshot = productConversationDetails[conversation.aggregateIdentity]?.snapshot else {
            return "Open the conversation before closing it."
        }
        guard snapshot.segments.count == 1 else {
            return "Close is unavailable for continued conversations."
        }
        return nil
    }

    @discardableResult
    func archive(conversationId: String) async -> Bool {
        guard ClientOperation.archive.policy == .onlineOnly else { return false }
        let serverIdentifiesCoordinator = conversationId == coordinatorConversationId
            || listStore.conversations.first {
                $0.transcriptRowIdentity == conversationId
            }?.isCoordinator == true
        guard conversationId != coordinatorConversationId, !serverIdentifiesCoordinator else {
            lastActionError = "The Coordinator is a permanent fleet conversation and can't be archived."
            return false
        }
        guard let api, connectivity.isOnline else {
            lastActionError = "Archiving needs a connection — it can't be queued."
            return false
        }
        let aggregateId = listStore.aggregateId(forTranscriptRowId: conversationId)
            ?? productConversationDetails.first(where: {
                $0.value.aggregateMemberTranscriptRowIds.contains(conversationId)
            })?.key
        if let aggregateId {
            guard productConversationDetails[aggregateId]?.snapshot?.segments.count == 1 else {
                lastActionError = "Close is unavailable for continued conversations."
                return false
            }
        }
        let archiveConversationId = aggregateId.flatMap { aggregate in
            productConversationDetails[aggregate]?.snapshot?.canonical_root.transcript_row_id
                ?? listStore.conversations.first(where: { $0.aggregateIdentity == aggregate })?.id
        } ?? conversationId
        let memberIds = aggregateId.map {
            authoritativeAggregateMemberIds(
                aggregateId: $0,
                triggeringTranscriptRowId: conversationId)
        } ?? [conversationId]
        for memberId in memberIds {
            if sessions[memberId]?.outbox.visibleEntries.isEmpty == false {
                lastActionError =
                    "This conversation has queued or unconfirmed messages. Retry or discard them before archiving."
                return false
            }
            if let session = sessions[memberId] {
                _ = await session.outbox.flushPersistence()
            }
            switch persistedOutboxContents(for: memberId) {
            case .empty:
                break
            case .hasVisibleEntries:
                lastActionError =
                    "This conversation has queued or unconfirmed messages. Retry or discard them before archiving."
                return false
            case .inaccessible:
                lastActionError =
                    "This conversation's queued-message store can't be read by this app version. Upgrade or clear the cache before archiving."
                return false
            }
        }
        guard let session = session(for: conversationId), session.beginArchiving() else {
            lastActionError =
                "This conversation has queued or unconfirmed messages. Retry or discard them before archiving."
            return false
        }
        var archived = false
        defer {
            if !archived { session.endArchiving() }
        }
        do {
            try await api.archive(conversationId: archiveConversationId)
            archived = true
            session.stop()
            await session.clearCachedSnapshotAndWait()
            await session.outbox.clearAndWait()
            sessions[conversationId] = nil
            if let aggregateId {
                listStore.remove(aggregateId: aggregateId)
            }
            let notificationId = aggregateId ?? conversationId
            UNUserNotificationCenter.current().removeDeliveredNotifications(
                withIdentifiers: ["attention-\(notificationId)"])
            UNUserNotificationCenter.current().removePendingNotificationRequests(
                withIdentifiers: ["attention-\(notificationId)"])
            return true
        } catch {
            lastActionError = (error as? APIError)?.errorDescription
                ?? error.localizedDescription
            return false
        }
    }

    func foregrounded() {
        scheduleDeliveryTrigger(.foreground)
        Task { await refreshList() }
    }

    private func scheduleDeliveryTrigger(_ trigger: DeliveryTrigger) {
        if persistedOutboxHydrated {
            for session in sessions.values {
                switch trigger {
                case .connectivityRestore:
                    session.resyncAfterConnectivityRestore()
                case .foreground:
                    session.resyncAfterForeground()
                }
            }
            schedulePersistedOutboxDrain()
        } else {
            Task { await resumeAfterDeliveryTrigger(trigger) }
        }
    }

    func integrateBackgroundConversationUpdate(existing: Conversation, update: Conversation) -> Conversation {
        if let aggregateIdentity = existing.product_conversation_id {
            return mergeAggregateProjection(
                existing: existing,
                liveUpdate: update,
                aggregateIdentity: aggregateIdentity)
        }
        return update
    }

    /// Deliver queued messages for conversations the user hasn't reopened.
    /// After a cold restart `sessions` is empty, so without this sweep an
    /// outbox persisted under `outbox-<id>.json` would sit on disk until
    /// its conversation was opened manually — breaking the restart-survival
    /// half of the offline queue. Sessions created here don't start an SSE
    /// stream; they exist to drain (their outbox reconciles on next open).
    @discardableResult
    private func drainPersistedOutboxes(
        authorityGeneration: Int,
        apiIdentity: APIConfigurationIdentity?
    ) async -> [String] {
        guard persistedOutboxDrainAuthorityGeneration == authorityGeneration,
              api?.configurationIdentity == apiIdentity,
              let api
        else { return [] }
        var drainedConversationIds: [String] = []
        let currentAPIIdentity = api.configurationIdentity
        let candidateConversationIds = await conversationPersistenceStore.pendingOutboxOwnerTranscriptRowIds(
            scope: api.configurationIdentity.persistenceScope)
        guard persistedOutboxDrainAuthorityGeneration == authorityGeneration,
              currentAPIIdentity == apiIdentity
        else { return drainedConversationIds }
        for conversationId in candidateConversationIds.sorted() {
            guard persistedOutboxDrainAuthorityGeneration == authorityGeneration,
                  currentAPIIdentity == apiIdentity
            else { return drainedConversationIds }
            guard sessions[conversationId] == nil else {
                continue
            }
            guard !hardDeletedConversationIds.contains(conversationId) else {
                continue
            }
            let drainSession: ConversationSession
            if let existing = drainSessions[conversationId] {
                drainSession = existing
            } else {
                drainSession = ConversationSession(
                    conversationId: conversationId,
                    api: api,
                    connectivity: connectivity,
                    outboxPersistence: conversationPersistenceStore.outboxPersistence(
                    conversationId: conversationId,
                    aggregateAuthority: aggregateIdentity(forTranscriptRowId: conversationId),
                    scope: api.configurationIdentity.persistenceScope),
                    snapshotPersistence: conversationPersistenceStore.snapshotPersistence(conversationId: conversationId),
                    retryTiming: LiveSessionTiming(),
                    staleCheckTiming: LiveSessionTiming(),
                    deliveryTriggerAllowed: { [weak self] in
                        self?.persistedOutboxHydrated == true
                    },
                    legacySnapshotPersistenceScope: legacySnapshotPersistenceScope,
                    aggregateAuthority: aggregateIdentity(forTranscriptRowId: conversationId))
                drainSessions[conversationId] = drainSession
            }
            drainedConversationIds.append(conversationId)
        }
        return drainedConversationIds
    }

    func backgrounded() {
        // Streams die in the background anyway; stop them cleanly and
        // persist snapshots. Outboxes are already disk-backed.
        for session in sessions.values { session.pauseForBackground() }
        if backgroundNudgesEnabled {
            BackgroundRefresh.scheduleNext()
        }
    }

    /// Sign-out also clears all cached data: conversations, the last-used
    /// working directory, and the pinned certificate are per-server state
    /// and must not leak across a server/account switch.
    func signOut() async {
        cancelPersistedOutboxDrainAuthority()
        apiGeneration += 1
        nudgePreferenceGeneration &+= 1
        backgroundNudgesEnabled = false
        nudgeAuthorizationHint = nil
        UserDefaults.standard.removeObject(forKey: Self.nudgesEnabledKey)
        BackgroundRefresh.cancelPending()
        pendingOpenConversationId = nil
        let notificationCenter = UNUserNotificationCenter.current()
        notificationCenter.removeAllDeliveredNotifications()
        notificationCenter.removeAllPendingNotificationRequests()
        UserDefaults.standard.removeObject(forKey: Self.lastCwdKey)
        if let configurationIdentity = api?.configurationIdentity {
            coordinatorIdentityStore.clear(configurationIdentity: configurationIdentity)
        }
        coordinatorConversationId = nil
        CertPinStore.forget()
        password = ""
        credentialStore.deleteRecord(account: Self.credentialRecordAccount)
        credentialStore.deleteRecord(account: Self.legacyPasswordAccount)
        credentialGeneration = Self.mintedCredentialGeneration()
        serverURLString = ""
        trustSelfSigned = false
        let ownedSessions = resetLocalStateForSignOut()
        await removeLocalStateForSignOut(ownedSessions: ownedSessions)
    }

    private func resetLocalStateForSignOut() -> [ConversationSession] {
        let cachedDetails = Array(productConversationDetails.values)
        productConversationDetails.removeAll()
        for detail in cachedDetails { detail.invalidateConfiguration() }
        let ownedSessions = Array(sessions.values) + Array(drainSessions.values)
        for session in ownedSessions { session.stop() }
        sessions.removeAll()
        drainSessions.removeAll()
        attention.reset()
        return ownedSessions
    }

    private func removeLocalStateForSignOut(ownedSessions: [ConversationSession]) async {
        for session in ownedSessions { await session.clearCachedSnapshotAndWait() }
        for session in ownedSessions { await session.outbox.clearAndWait() }
        async let resetConversationListCache: Void = listStore.reset()
        async let removeAllPersistedConversationState: Void = conversationPersistenceStore.removeAllPersistedConversationState()
        _ = await (resetConversationListCache, removeAllPersistedConversationState)
        coordinatorConversationId = nil
    }

    #if DEBUG
    static func resetPersistentStateForUITesting() {
        if let bundleIdentifier = Bundle.main.bundleIdentifier {
            UserDefaults.standard.removePersistentDomain(forName: bundleIdentifier)
        }
        KeychainCredentialStore().deleteRecord(account: Self.credentialRecordAccount)
        KeychainCredentialStore().deleteRecord(account: Self.legacyPasswordAccount)
        DiskStore.removeAll()
        let center = UNUserNotificationCenter.current()
        center.removeAllDeliveredNotifications()
        center.removeAllPendingNotificationRequests()
    }
    #endif
}

/// Routes notification taps into the app (deep link to the conversation)
/// and suppresses banners while the app is foregrounded — the user is
/// already looking at live state.
final class NotificationRouter: NSObject, UNUserNotificationCenterDelegate {
    weak var model: AppModel?

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let conversationId =
            response.notification.request.content.userInfo["conversationId"] as? String
        Task { @MainActor [weak model] in
            if let conversationId {
                model?.pendingOpenConversationId = conversationId
            }
            completionHandler()
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([])
    }
}
