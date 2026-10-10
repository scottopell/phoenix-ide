import XCTest

@testable import PhoenixMobile

final class KeychainCredentialTests: XCTestCase {
    private let origin = "https://phoenix.invalid"

    func testLegacyMigrationPersistsRandomGenerationAndProvenanceAcrossLaunches() throws {
        var items = [Keychain.legacyPasswordAccount: "weak-password"]
        let record = try Keychain.loadCredential(
            persistedServerURL: origin,
            read: { items[$0] },
            write: { items[$1] = $0 },
            delete: { items.removeValue(forKey: $0) })
        XCTAssertEqual(record.password, "weak-password")
        XCTAssertNil(items[Keychain.legacyPasswordAccount])
        let reloaded = try Keychain.loadCredential(
            persistedServerURL: origin,
            read: { items[$0] },
            write: { _, _ in XCTFail("Must not rewrite the generation") },
            delete: { _ in XCTFail("Must not remigrate") })
        XCTAssertEqual(record, reloaded)
        XCTAssertEqual(
            reloaded.legacyPersistenceScope(serverURL: URL(string: origin)!),
            "\(origin)|\(record.generation.uuidString)")
        XCTAssertNil(reloaded.legacyPersistenceScope(serverURL: URL(string: "https://other.invalid")!))
        XCTAssertNotEqual(record.generation, Keychain.CredentialRecord(password: record.password).generation)
    }

    func testFailedMigrationDoesNotPublishGenerationOrDeleteLegacyPassword() {
        enum Failure: Error { case write }
        var deleted = false
        XCTAssertThrowsError(try Keychain.loadCredential(
            persistedServerURL: origin,
            read: { $0 == Keychain.legacyPasswordAccount ? "password" : nil },
            write: { _, _ in throw Failure.write },
            delete: { _ in deleted = true }))
        XCTAssertFalse(deleted)
    }

    func testMissingLegacyCredentialCannotProveLegacyScope() throws {
        let record = try Keychain.loadCredential(
            persistedServerURL: origin,
            read: { _ in nil }, write: { _, _ in }, delete: { _ in XCTFail() })
        XCTAssertNil(record.legacyPersistenceScope(serverURL: URL(string: origin)!))
    }

    func testEmptyLegacyPasswordStillProvesPasswordlessInstallation() throws {
        let record = try Keychain.loadCredential(
            persistedServerURL: origin,
            read: { $0 == Keychain.legacyPasswordAccount ? "" : nil },
            write: { _, _ in }, delete: { _ in })
        XCTAssertNotNil(record.legacyPersistenceScope(serverURL: URL(string: origin)!))
    }

    func testMissingPersistedServerDoesNotProveLegacyScope() throws {
        let record = try Keychain.loadCredential(
            persistedServerURL: "",
            read: { $0 == Keychain.legacyPasswordAccount ? "password" : nil },
            write: { _, _ in }, delete: { _ in })
        XCTAssertNil(record.legacyServerURL)
    }

    func testCredentialReadFailureDoesNotMintReplacementGeneration() {
        enum Failure: Error { case read }
        XCTAssertThrowsError(try Keychain.loadCredential(
            persistedServerURL: origin,
            read: { _ in throw Failure.read },
            write: { _, _ in XCTFail("Must not write after an unreadable Keychain item") },
            delete: { _ in XCTFail() }))
    }

    func testInvalidAndNewerRecordsFailClosedWithoutLegacyFallback() throws {
        let record = Keychain.CredentialRecord(password: "password")
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(record)) as? [String: Any])
        object["version"] = 2
        let newer = String(decoding: try JSONSerialization.data(withJSONObject: object), as: UTF8.self)
        for encoded in ["invalid-json", newer] {
            XCTAssertThrowsError(try Keychain.loadCredential(
                persistedServerURL: origin,
                read: { account in
                    XCTAssertEqual(account, Keychain.credentialAccount)
                    return encoded
                },
                write: { _, _ in XCTFail() }, delete: { _ in XCTFail() }))
        }
    }
}
