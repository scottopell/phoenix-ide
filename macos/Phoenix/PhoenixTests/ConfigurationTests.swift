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

    func testBundledEnvironmentIsLoopbackTLSOffAndPrivate() throws {
        let root = URL(fileURLWithPath: "/tmp/Phoenix Tests")
        let configuration = BundledServerConfiguration(
            origin: try PhoenixOrigin("http://127.0.0.1:8420"),
            executableURL: root.appendingPathComponent("phoenix_ide"),
            runtimeRootURL: root,
            dataDirectoryURL: root.appendingPathComponent(".phoenix-ide"),
            databaseURL: root.appendingPathComponent(".phoenix-ide/phoenix.db"),
            logURL: root.appendingPathComponent(".phoenix-ide/phoenix.log"),
            ownerLockURL: root.appendingPathComponent(".phoenix-ide/owner.lock"),
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
            runtimeRootURL: root,
            dataDirectoryURL: root.appendingPathComponent(".phoenix-ide"),
            databaseURL: root.appendingPathComponent(".phoenix-ide/phoenix.db"),
            logURL: root.appendingPathComponent(".phoenix-ide/phoenix.log"),
            ownerLockURL: root.appendingPathComponent(".phoenix-ide/owner.lock"),
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

    func testLogBufferPreservesNonUTF8BytesLossilyWithoutDroppingLine() {
        var buffer = ConnectionLogBuffer(maxLines: 4)
        let redact: (String) -> String = { $0 }
        let emitted = buffer.append(Data([0x66, 0x6F, 0x80, 0x6F, 0x0A]), redact: redact)

        XCTAssertEqual(emitted, ["fo�o"])
        XCTAssertEqual(buffer.completeLines, ["fo�o"])
        XCTAssertEqual(buffer.pendingFragment, "")
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

    func testDeploymentViolationCanLeaveAttachedDeploymentViewableButReadOnly() throws {
        let version = VersionInfo(version: "1.2.3", gitSHA: "abc123")
        let deployment = DeploymentInfo(
            build: BuildInfo(version: "1.2.3", gitSHA: "abc123"),
            network: NetworkInfo(bindAddress: "127.0.0.1:8031", socketActivated: false, tls: TLSInfo(enabled: false, mode: nil)),
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
}
