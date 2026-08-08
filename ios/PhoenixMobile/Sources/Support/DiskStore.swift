import Foundation

/// JSON file persistence under Application Support. All offline state —
/// conversation list, per-conversation snapshots, outboxes — goes through
/// here so the app renders instantly with no network.
///
/// ## The versioning rule (REQ-IOS-014)
///
/// Durable stores use `saveVersioned`/`loadVersioned`, which wrap the
/// payload in `{schema_version, payload}`. **Changing any persisted struct
/// requires one of:**
///   1. bumping that store's schema version constant and adding a branch
///      to its `migrate` closure that upgrades the old payload, OR
///   2. a comment on the changed field noting it is additive-optional
///      (old files decode it as nil/default — no bump owed).
/// Without this, a shape change makes old files undecodable and `try?`
/// silently wipes the cache — for the outbox, that is queued-message loss.
///
/// Load semantics: same version → decode; older version → the store's
/// migrate hook; **newer** version (downgraded app) → refuse and treat as
/// absent rather than misparse; a pre-envelope legacy file decodes as the
/// bare payload (version 0).
///
/// MainActor-isolated: every caller (stores, sessions, AppModel) is already
/// MainActor, and isolation is what makes the mutable `baseDirectory` test
/// seam safe.
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

    private static func url(for name: String) -> URL {
        directory.appendingPathComponent(name + ".json")
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
        let destination = url(for: name)
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
}
