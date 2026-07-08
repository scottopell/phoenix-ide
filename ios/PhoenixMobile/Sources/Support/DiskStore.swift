import Foundation

/// JSON file persistence under Application Support. All offline state —
/// conversation list, per-conversation snapshots, outboxes — goes through
/// here so the app renders instantly with no network.
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

    static func save<T: Encodable>(_ value: T, name: String) {
        guard let data = try? JSONEncoder().encode(value) else { return }
        // Atomic write: a crash mid-write must not corrupt cached state.
        try? data.write(to: url(for: name), options: .atomic)
    }

    static func remove(name: String) {
        try? FileManager.default.removeItem(at: url(for: name))
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
