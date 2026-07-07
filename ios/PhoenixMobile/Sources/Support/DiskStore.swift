import Foundation

/// JSON file persistence under Application Support. All offline state —
/// conversation list, per-conversation snapshots, outboxes — goes through
/// here so the app renders instantly with no network.
enum DiskStore {
    private static var directory: URL {
        let base = FileManager.default.urls(
            for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = base.appendingPathComponent("PhoenixMobile", isDirectory: true)
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

    static func removeAll() {
        try? FileManager.default.removeItem(at: directory)
    }
}
