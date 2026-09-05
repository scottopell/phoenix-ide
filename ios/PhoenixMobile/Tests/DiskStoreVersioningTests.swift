import XCTest

@testable import PhoenixMobile

// Contract tests for the versioned persistence envelope (REQ-IOS-014,
// DiskStore versioning rule): one test per load-semantics rule. This is
// the net under every future persisted-struct change — a shape change
// without a version bump or an additive-optional note should make one of
// these fail conceptually, not wipe a user's outbox silently.
final class DiskStoreVersioningTests: XCTestCase {

    @MainActor
    private func freshDiskStore() {
        DiskStore.baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-versioning-tests-\(UUID().uuidString)")
    }

    private struct Record: Codable, Equatable, Sendable {
        var name: String
        var count: Int
    }

    @MainActor
    func testSameVersionRoundTrips() {
        freshDiskStore()
        let value = [Record(name: "a", count: 1), Record(name: "b", count: 2)]
        XCTAssertTrue(DiskStore.saveVersioned(value, name: "records", version: 3))
        let loaded = DiskStore.loadVersioned([Record].self, name: "records", version: 3)
        XCTAssertEqual(loaded, value)
    }

    @MainActor
    func testLegacyBareArrayFileLoadsAsVersionZero() {
        // Files written before the envelope existed store the bare payload;
        // they must keep loading (this is the real migration path for every
        // pre-envelope install).
        freshDiskStore()
        let value = [Record(name: "legacy", count: 7)]
        DiskStore.save(value, name: "records")  // bare, unversioned
        let loaded = DiskStore.loadVersioned([Record].self, name: "records", version: 1)
        XCTAssertEqual(loaded, value)
    }

    @MainActor
    func testLegacyBareObjectFileLoadsAsVersionZero() {
        // Object-shaped legacy payloads (e.g. Snapshot) lack the
        // schema_version key entirely — also the legacy path.
        freshDiskStore()
        let value = Record(name: "snapshot", count: 1)
        DiskStore.save(value, name: "snap")
        let loaded = DiskStore.loadVersioned(Record.self, name: "snap", version: 1)
        XCTAssertEqual(loaded, value)
    }

    @MainActor
    func testNewerVersionIsRefusedNotMisparsed() {
        // A downgraded app must not guess at a newer schema — treat the
        // file as absent (and never delete it on load).
        freshDiskStore()
        XCTAssertTrue(DiskStore.saveVersioned(
            [Record(name: "future", count: 9)], name: "records", version: 99))
        XCTAssertNil(DiskStore.loadVersioned([Record].self, name: "records", version: 1))
    }

    @MainActor
    func testDowngradedWriterCannotOverwriteNewerStore() {
        freshDiskStore()
        let future = [Record(name: "future", count: 99)]
        XCTAssertTrue(DiskStore.saveVersioned(future, name: "records", version: 9))

        XCTAssertFalse(DiskStore.saveVersioned(
            [Record(name: "downgraded", count: 1)], name: "records", version: 1))
        XCTAssertEqual(
            DiskStore.loadVersioned([Record].self, name: "records", version: 9),
            future)
    }

    @MainActor
    func testOlderVersionRoutesThroughMigrateHook() {
        freshDiskStore()
        // v1 stored shape: [String]. Current (v2) shape: [Record].
        XCTAssertTrue(DiskStore.saveVersioned(["a", "b"], name: "records", version: 1))

        var sawVersion: Int?
        let loaded = DiskStore.loadVersioned(
            [Record].self, name: "records", version: 2,
            migrate: { storedVersion, fileData in
                sawVersion = storedVersion
                struct V1Envelope: Decodable {
                    var payload: [String]
                }
                guard let old = try? JSONDecoder().decode(V1Envelope.self, from: fileData)
                else { return nil }
                return old.payload.map { Record(name: $0, count: 0) }
            })

        XCTAssertEqual(sawVersion, 1)
        XCTAssertEqual(loaded, [Record(name: "a", count: 0), Record(name: "b", count: 0)])
    }

    @MainActor
    func testUndecodableLegacyFileRoutesToMigrateAsVersionZero() {
        freshDiskStore()
        // Legacy bare file whose shape the current type can't decode.
        DiskStore.save(["just", "strings"], name: "records")
        var sawVersion: Int?
        let loaded = DiskStore.loadVersioned(
            [Record].self, name: "records", version: 1,
            migrate: { storedVersion, _ in
                sawVersion = storedVersion
                return []
            })
        XCTAssertEqual(sawVersion, 0)
        XCTAssertEqual(loaded, [])
    }

    @MainActor
    func testOlderVersionWithoutMigrateHookTreatsFileAsAbsent() {
        freshDiskStore()
        XCTAssertTrue(DiskStore.saveVersioned(["a"], name: "records", version: 1))
        XCTAssertNil(DiskStore.loadVersioned([Record].self, name: "records", version: 2))
    }

    @MainActor
    func testSignOutResetFencesPendingHardDeleteFenceSaveBeforePublication() async throws {
        let baseDirectory = FileManager.default.temporaryDirectory
            .appendingPathComponent("phoenix-signout-fence-\(UUID().uuidString)")
        let context = DiskStore.versionedContext(baseDirectory: baseDirectory)
        let destination = DiskStore.phoenixMobileDirectory(baseDirectory: baseDirectory)
            .appendingPathComponent("hard-delete-pending")
            .appendingPathExtension("json")
        let writer = context.writer(destinationURL: destination, version: 1)
        let pendingSaveRevision = writer.reserveRevision()
        let fence = PersistedHardDeleteFence(
            persistenceScope: .init(serverURL: "https://phoenix.invalid", credentialGeneration: "credential"),
            aggregateAuthority: "pc-1",
            memberConversationIds: ["row-1"])

        await context.removeAllAndWait()
        let lateSaveCompleted = await writer.save(fence, revision: pendingSaveRevision)

        XCTAssertTrue(lateSaveCompleted)
        XCTAssertFalse(FileManager.default.fileExists(atPath: destination.path))
    }

    @MainActor
    func testWriterHandlesShareOneDestinationRevisionFence() async {
        freshDiskStore()
        let context = DiskStore.versionedContext()
        let first = context.writer(name: "records", version: 1)
        let replacement = context.writer(name: "records", version: 1)
        let oldRevision = first.reserveRevision()
        let newRevision = replacement.reserveRevision()

        let replacementSaved = await replacement.save(
            [Record(name: "new", count: 2)], revision: newRevision)
        let olderSaveWasFenced = await first.save(
            [Record(name: "old", count: 1)], revision: oldRevision)
        XCTAssertTrue(replacementSaved)
        XCTAssertTrue(olderSaveWasFenced)

        XCTAssertEqual(
            DiskStore.loadVersioned([Record].self, name: "records", version: 1),
            [Record(name: "new", count: 2)])
    }
}
