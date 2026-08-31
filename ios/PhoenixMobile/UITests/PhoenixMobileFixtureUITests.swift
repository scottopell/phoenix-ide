import XCTest

final class PhoenixMobileFixtureUITests: XCTestCase {
    private let app = XCUIApplication()

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testFixtureCatalogLaunchesOfflineAndNormalScenarios() throws {
        launchFixture(.catalog)

        XCTAssertTrue(element("fixture.catalog").waitForExistence(timeout: 10))
        attachScreenshot(named: "fixture-catalog")

        element("fixture.catalog.offline").tap()
        XCTAssertTrue(element("fixture.ready.offline").waitForExistence(timeout: 10))
        XCTAssertTrue(element("fixture.offlineBanner").exists)
        XCTAssertTrue(element("fixture.stateDetail").exists)
        attachScreenshot(named: "fixture-offline")
        XCTAssertTrue(element("state.questionCard").exists)
        XCTAssertTrue(scrollFixtureShell(to: element("fixture.composer")))
        XCTAssertTrue(element("conversation.send").isEnabled)

        app.terminate()
        launchFixture(.normal)
        XCTAssertTrue(element("fixture.ready.normal").waitForExistence(timeout: 10))
        XCTAssertTrue(scrollTranscript(to: element("message.user")))
        XCTAssertTrue(scrollTranscript(to: element("message.agent")))
        XCTAssertTrue(scrollTranscript(to: element("tool.think")))
        XCTAssertTrue(scrollTranscript(to: element("tool.bash")))
        XCTAssertTrue(scrollTranscript(to: element("tool.bashResult")))
        XCTAssertTrue(scrollFixtureShell(to: element("message.outbox")))
        XCTAssertTrue(scrollFixtureShell(to: element("fixture.composer")))
        XCTAssertTrue(element("conversation.send").isEnabled)
        attachScreenshot(named: "fixture-normal")
    }

    func testMalformedAndErrorFixturesShowFallbackSurfaces() throws {
        launchFixture(.malformed)

        XCTAssertTrue(element("fixture.ready.malformed").waitForExistence(timeout: 10))
        XCTAssertTrue(scrollTranscript(to: element("tool.genericUse")))
        XCTAssertTrue(scrollTranscript(to: element("tool.genericResult")))
        XCTAssertTrue(app.staticTexts.containing(NSPredicate(format: "label CONTAINS %@", "future_block")).firstMatch.exists)
        attachScreenshot(named: "fixture-malformed")

        app.terminate()
        launchFixture(.error)
        XCTAssertTrue(element("fixture.ready.error").waitForExistence(timeout: 10))
        XCTAssertTrue(element("fixture.storageWarning").exists)
        XCTAssertTrue(scrollTranscript(to: element("tool.bashResult")))
        XCTAssertTrue(element("fixture.stateDetail").exists)
        attachScreenshot(named: "fixture-error")
    }

    func testShellStateFixturesAreIndependentlyInspectable() throws {
        for fixture in [FixtureID.loading, .empty, .cached, .readOnly] {
            app.terminate()
            launchFixture(fixture)
            XCTAssertTrue(element("fixture.ready.\(fixture.rawValue)").waitForExistence(timeout: 10))
            XCTAssertTrue(scrollFixtureShell(to: element("fixture.stateDetail")))
            if fixture == .readOnly {
                let approval = element("state.taskApprovalCard")
                XCTAssertTrue(scrollFixtureShell(to: approval))
                XCTAssertFalse(app.buttons["Approve…"].isEnabled)
                XCTAssertTrue(scrollFixtureShell(to: element("fixture.composer")))
                XCTAssertFalse(element("conversation.send").isEnabled)
            }
            attachScreenshot(named: "fixture-\(fixture.rawValue)")
        }
    }

    private func launchFixture(_ fixture: FixtureID) {
        app.launchArguments = ["-fixture", fixture.rawValue]
        app.launch()
    }

    private func scrollFixtureShell(to target: XCUIElement) -> Bool {
        let shell = app.scrollViews.firstMatch
        guard shell.waitForExistence(timeout: 5) else { return false }
        for _ in 0..<8 {
            if target.exists && target.isHittable { return true }
            shell.swipeUp()
        }
        return target.exists
    }

    private func scrollTranscript(to target: XCUIElement) -> Bool {
        let transcript = app.scrollViews.element(boundBy: 1)
        guard transcript.waitForExistence(timeout: 5) else { return false }
        transcript.swipeDown()
        for _ in 0..<8 {
            if target.exists && target.isHittable { return true }
            transcript.swipeUp()
        }
        return target.exists
    }

    private func element(_ identifier: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    private func attachScreenshot(named name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private enum FixtureID: String {
        case catalog
        case normal
        case loading
        case empty
        case malformed
        case error
        case offline
        case cached
        case readOnly = "read-only"
    }
}
