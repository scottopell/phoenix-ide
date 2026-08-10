import XCTest
@testable import PhoenixMacCore

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

    func testBundledEnvironmentIsLoopbackTLSOffAndPrivate() throws {
        let root = URL(fileURLWithPath: "/tmp/Phoenix Tests")
        let configuration = BundledServerConfiguration(
            origin: try PhoenixOrigin("http://127.0.0.1:8420"),
            executableURL: root.appendingPathComponent("phoenix_ide"),
            databaseURL: root.appendingPathComponent("phoenix.db"),
            logURL: root.appendingPathComponent("phoenix.log"),
            ownerLockURL: root.appendingPathComponent("owner.lock"),
            rustLogLevel: "phoenix_ide=debug"
        )
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_BIND_ADDR"], "127.0.0.1")
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_TLS"], "off")
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_PORT"], "8420")
        XCTAssertEqual(configuration.publicEnvironment["PHOENIX_DB_PATH"], "/tmp/Phoenix Tests/phoenix.db")
        XCTAssertNil(configuration.publicEnvironment["ANTHROPIC_API_KEY"])
        XCTAssertNil(configuration.publicEnvironment["PHOENIX_PASSWORD"])
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
            databaseURL: root.appendingPathComponent("phoenix.db"),
            logURL: root.appendingPathComponent("phoenix.log"),
            ownerLockURL: root.appendingPathComponent("owner.lock"),
            rustLogLevel: "info"
        )
        let bundled = ServerMode.bundled(bundledConfig)
        XCTAssertEqual(bundled.kind, .bundled)
        XCTAssertEqual(bundled.origin.description, "http://127.0.0.1:8420")
    }
}
