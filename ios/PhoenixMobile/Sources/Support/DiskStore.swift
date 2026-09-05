import Foundation

/// Serial background sink for one destination. Every writer handle for the
/// destination shares this revision fence.
private actor VersionedDiskSink {
    private let destination: URL
    private var latestAttemptedRevision = 0
    private var latestCommittedRevision = 0

    init(destination: URL) {
        self.destination = destination
    }

    func save<T: Encodable & Sendable>(
        _ value: T, version: Int, revision: Int
    ) -> Bool {
        guard revision >= latestAttemptedRevision else {
            return revision <= latestCommittedRevision
        }
        latestAttemptedRevision = revision
        let committed = DiskStore.writeVersioned(value, to: destination, version: version)
        if committed {
            latestCommittedRevision = max(latestCommittedRevision, revision)
        }
        return committed
    }

    func remove(revision: Int) {
        guard revision >= latestAttemptedRevision else { return }
        latestAttemptedRevision = revision
        do {
            try FileManager.default.removeItem(at: destination)
        } catch {
            guard (error as NSError).code == NSFileNoSuchFileError else { return }
        }
        latestCommittedRevision = max(latestCommittedRevision, revision)
    }
}

@MainActor
private final class VersionedDiskDestination {
    let sink: VersionedDiskSink
    let destinationURL: URL
    private var nextRevision = 0

    init(destination: URL) {
        destinationURL = destination
        sink = VersionedDiskSink(destination: destination)
    }

    func reserveRevision() -> Int {
        nextRevision += 1
        return nextRevision
    }
}

/// Main-actor handle that reserves logical revisions before work leaves the
/// actor, while encoding and file I/O run in the shared background sink.
@MainActor
final class VersionedDiskWriter {
    private let destination: VersionedDiskDestination
    private let version: Int

    var destinationURL: URL { destination.destinationURL }

    fileprivate init(destination: VersionedDiskDestination, version: Int) {
        self.destination = destination
        self.version = version
    }

    func reserveRevision() -> Int {
        destination.reserveRevision()
    }

    func save<T: Encodable & Sendable>(_ value: T, revision: Int) async -> Bool {
        await destination.sink.save(value, version: version, revision: revision)
    }

    func remove(revision: Int) async {
        await destination.sink.remove(revision: revision)
    }
}

@MainActor
final class VersionedDiskContext {
    let rootDirectory: URL
    private var destinations: [URL: VersionedDiskDestination] = [:]

    init(rootDirectory: URL) {
        self.rootDirectory = rootDirectory.standardizedFileURL
    }

    func writer(destinationURL: URL, version: Int) -> VersionedDiskWriter {
        let normalizedURL = destinationURL.standardizedFileURL
        let destination = destinations[normalizedURL] ?? VersionedDiskDestination(destination: normalizedURL)
        destinations[normalizedURL] = destination
        return VersionedDiskWriter(destination: destination, version: version)
    }

    func writer(name: String, version: Int) -> VersionedDiskWriter {
        writer(destinationURL: rootDirectory.appendingPathComponent(name).appendingPathExtension("json"), version: version)
    }

    func removeAllAndWait() async {
        let removals = destinations.values.compactMap { destination -> (VersionedDiskSink, Int)? in
            let destinationURL = destination.destinationURL.standardizedFileURL
            guard destinationURL.path.hasPrefix(rootDirectory.path + "/") else { return nil }
            return (destination.sink, destination.reserveRevision())
        }
        for (sink, revision) in removals {
            await sink.remove(revision: revision)
        }
        try? FileManager.default.removeItem(at: rootDirectory)
    }
}

/// Main-actor JSON persistence under Application Support. Versioned loads
/// decode matching envelopes, delegate older payloads to the supplied
/// migration, reject newer envelopes, and accept bare legacy payloads as v0.
@MainActor
enum DiskStore {
    /// Test seam: contract tests point this at a fresh temp directory so
    /// they never touch (or depend on) the app's real cache.
    static var baseDirectory: URL = FileManager.default.urls(
        for: .applicationSupportDirectory, in: .userDomainMask)[0]


    private static var directory: URL {
        let dir = baseDirectory.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    static func url(for name: String) -> URL {
        directory.appendingPathComponent(name + ".json")
    }
    static func listNames(prefix: String) -> [String] {
        guard let files = try? FileManager.default.contentsOfDirectory(
            at: directory,
            includingPropertiesForKeys: nil)
        else { return [] }
        return files
            .filter { $0.lastPathComponent.hasPrefix(prefix) && $0.pathExtension == "json" }
            .map { $0.deletingPathExtension().lastPathComponent }
    }

    static func load<T: Decodable>(_ type: T.Type, name: String) -> T? {
        guard let data = try? Data(contentsOf: url(for: name)) else { return nil }
        return try? JSONDecoder().decode(type, from: data)
    }

    /// Returns whether the write actually reached disk. Cache callers may
    /// ignore this (stale cache self-heals); the outbox must not — its
    /// enqueue-before-POST durability point is only real if the write was.
    @discardableResult
    static func save<T: Encodable>(_ value: T, name: String) -> Bool {
        guard let data = try? JSONEncoder().encode(value) else { return false }
        do {
            // Atomic write: a crash mid-write must not corrupt cached state.
            try data.write(to: url(for: name), options: .atomic)
            return true
        } catch {
            return false
        }
    }

    static func remove(name: String) {
        try? FileManager.default.removeItem(at: url(for: name))
    }

    // MARK: - Versioned stores (see the versioning rule above)

    private struct SaveEnvelope<T: Encodable>: Encodable {
        var schema_version: Int
        var payload: T
    }

    private struct LoadEnvelope<T: Decodable>: Decodable {
        var schema_version: Int
        var payload: T
    }

    /// Probe for the envelope discriminator. Decoding this against a bare
    /// legacy payload either throws (arrays) or yields nil (objects
    /// without the key) — both route to the legacy path.
    private struct VersionProbe: Decodable {
        var schema_version: Int?
    }

    enum VersionedLoad<T> {
        case missing
        case value(T)
        case incompatible(storedVersion: Int)
        case unreadable
    }

    nonisolated static func phoenixMobileDirectory(baseDirectory: URL) -> URL {
        baseDirectory.appendingPathComponent("PhoenixMobile", isDirectory: true)
    }

    static func versionedContext(baseDirectory: URL? = nil) -> VersionedDiskContext {
        let resolvedBaseDirectory = baseDirectory ?? self.baseDirectory
        return VersionedDiskContext(rootDirectory: phoenixMobileDirectory(baseDirectory: resolvedBaseDirectory))
    }

    nonisolated static func names(in directory: URL, withPrefix prefix: String) -> [String] {
        let urls = (try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil)) ?? []
        return urls.compactMap { url in
            guard url.pathExtension == "json" else { return nil }
            let name = url.deletingPathExtension().lastPathComponent
            return name.hasPrefix(prefix) ? name : nil
        }
    }

    nonisolated static func loadVersionedResult<T: Decodable>(
        _ type: T.Type,
        source: URL,
        version: Int,
        migrate: ((_ storedVersion: Int, _ fileData: Data) -> T?)? = nil
    ) -> VersionedLoad<T> {
        guard let data = try? Data(contentsOf: source) else {
            return FileManager.default.fileExists(atPath: source.path) ? .unreadable : .missing
        }

        return loadVersionedResult(type, fileData: data, version: version, migrate: migrate)
    }

    nonisolated static func loadVersionedResult<T: Decodable>(
        _ type: T.Type,
        fileData data: Data,
        version: Int,
        migrate: ((_ storedVersion: Int, _ fileData: Data) -> T?)? = nil
    ) -> VersionedLoad<T> {
        if let stored = (try? JSONDecoder().decode(VersionProbe.self, from: data))?
            .schema_version
        {
            if stored == version {
                guard let payload = try? JSONDecoder().decode(LoadEnvelope<T>.self, from: data)
                    .payload
                else { return .unreadable }
                return .value(payload)
            }
            if stored > version {
                return .incompatible(storedVersion: stored)
            }
            guard let migrated = migrate?(stored, data) else { return .unreadable }
            return .value(migrated)
        }

        if let bare = try? JSONDecoder().decode(T.self, from: data) {
            return .value(bare)
        }
        guard let migrated = migrate?(0, data) else { return .unreadable }
        return .value(migrated)
    }

    @discardableResult
    static func saveVersioned<T: Encodable>(_ value: T, name: String, version: Int) -> Bool {
        writeVersioned(value, to: url(for: name), version: version)
    }

    nonisolated static func encodeVersioned<T: Encodable>(_ value: T, version: Int) throws -> Data {
        try JSONEncoder().encode(SaveEnvelope(schema_version: version, payload: value))
    }

    nonisolated fileprivate static func writeVersioned<T: Encodable>(
        _ value: T,
        to destination: URL,
        version: Int
    ) -> Bool {
        do {
            try FileManager.default.createDirectory(
                at: destination.deletingLastPathComponent(),
                withIntermediateDirectories: true)
        } catch {
            return false
        }
        if let existing = try? Data(contentsOf: destination),
           let stored = (try? JSONDecoder().decode(VersionProbe.self, from: existing))?
            .schema_version,
           stored > version
        {
            // A downgraded app must never overwrite a store whose schema it
            // cannot understand. The user can upgrade or explicitly clear
            // the cache; ordinary writes stay fail-closed until then.
            return false
        }
        guard let data = try? encodeVersioned(value, version: version)
        else { return false }
        do {
            try data.write(to: destination, options: .atomic)
            return true
        } catch {
            return false
        }
    }

    /// Load a versioned store. `migrate` receives the stored version and
    /// the raw file bytes for any version older than `version` (including
    /// 0 for a legacy bare file the current shape can't decode); return
    /// nil to treat the file as unusable.
    static func loadVersioned<T: Decodable>(
        _ type: T.Type, name: String, version: Int,
        migrate: ((_ storedVersion: Int, _ fileData: Data) -> T?)? = nil
    ) -> T? {
        guard case .value(let value) = loadVersionedResult(
            type, name: name, version: version, migrate: migrate)
        else { return nil }
        return value
    }

    static func loadVersionedResult<T: Decodable>(
        _ type: T.Type, name: String, version: Int,
        migrate: ((_ storedVersion: Int, _ fileData: Data) -> T?)? = nil
    ) -> VersionedLoad<T> {
        loadVersionedResult(type, source: url(for: name), version: version, migrate: migrate)
    }

    /// Names (without extension) of stored files matching a prefix. Used to
    /// discover persisted per-conversation outboxes independently of which
    /// sessions are currently open.
    static func names(withPrefix prefix: String) -> [String] {
        names(in: directory, withPrefix: prefix)
    }

    static func removeAll() {
        try? FileManager.default.removeItem(at: directory)
    }

    static func removeDirectoryAndWait(_ directory: URL) async {
        await VersionedDiskContext(rootDirectory: directory).removeAllAndWait()
    }

    static func removeAllAndWait() async {
        await versionedContext().removeAllAndWait()
    }
}
