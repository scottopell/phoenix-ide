import Foundation

/// Serial background sink for one destination. Every writer handle for the
/// destination shares this revision fence.
private actor VersionedDiskSink {
    private let destination: URL
    private var latestRevision = 0

    init(destination: URL) {
        self.destination = destination
    }

    func save<T: Encodable & Sendable>(
        _ value: T, version: Int, revision: Int
    ) -> Bool {
        guard revision >= latestRevision else { return true }
        latestRevision = revision
        return DiskStore.writeVersioned(value, to: destination, version: version)
    }

    func remove(revision: Int) {
        guard revision >= latestRevision else { return }
        latestRevision = revision
        try? FileManager.default.removeItem(at: destination)
    }
}

@MainActor
private final class VersionedDiskDestination {
    let sink: VersionedDiskSink
    private var nextRevision = 0

    init(destination: URL) {
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

/// Main-actor JSON persistence under Application Support. Versioned loads
/// decode matching envelopes, delegate older payloads to the supplied
/// migration, reject newer envelopes, and accept bare legacy payloads as v0.
@MainActor
enum DiskStore {
    /// Test seam: contract tests point this at a fresh temp directory so
    /// they never touch (or depend on) the app's real cache.
    static var baseDirectory: URL = FileManager.default.urls(
        for: .applicationSupportDirectory, in: .userDomainMask)[0]

    private static var versionedDestinations: [URL: VersionedDiskDestination] = [:]

    private static var directory: URL {
        let dir = baseDirectory.appendingPathComponent("PhoenixMobile", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        return dir
    }

    private static func url(for name: String) -> URL {
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

    @discardableResult
    static func saveVersioned<T: Encodable>(_ value: T, name: String, version: Int) -> Bool {
        writeVersioned(value, to: url(for: name), version: version)
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
        guard let data = try? JSONEncoder().encode(
            SaveEnvelope(schema_version: version, payload: value))
        else { return false }
        do {
            try data.write(to: destination, options: .atomic)
            return true
        } catch {
            return false
        }
    }

    static func versionedWriter(name: String, version: Int) -> VersionedDiskWriter {
        let destinationURL = url(for: name)
        let destination = versionedDestinations[destinationURL]
            ?? VersionedDiskDestination(destination: destinationURL)
        versionedDestinations[destinationURL] = destination
        return VersionedDiskWriter(destination: destination, version: version)
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
        let source = url(for: name)
        guard let data = try? Data(contentsOf: source) else {
            return FileManager.default.fileExists(atPath: source.path) ? .unreadable : .missing
        }

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

        // Legacy pre-envelope file: the payload was stored bare.
        if let bare = try? JSONDecoder().decode(T.self, from: data) {
            return .value(bare)
        }
        guard let migrated = migrate?(0, data) else { return .unreadable }
        return .value(migrated)
    }

    /// Names (without extension) of stored files matching a prefix. Used to
    /// discover persisted per-conversation outboxes independently of which
    /// sessions are currently open.
    static func names(withPrefix prefix: String) -> [String] {
        let urls = (try? FileManager.default.contentsOfDirectory(
            at: directory, includingPropertiesForKeys: nil)) ?? []
        return urls.compactMap { url in
            guard url.pathExtension == "json" else { return nil }
            let name = url.deletingPathExtension().lastPathComponent
            return name.hasPrefix(prefix) ? name : nil
        }
    }

    static func removeAll() {
        try? FileManager.default.removeItem(at: directory)
    }

    static func removeAllAndWait() async {
        let removals = versionedDestinations.values.map { destination in
            (destination.sink, destination.reserveRevision())
        }
        for (sink, revision) in removals {
            await sink.remove(revision: revision)
        }
        removeAll()
    }
}
