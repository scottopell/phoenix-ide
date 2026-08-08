import XCTest

final class PhoenixMobileUIQATests: XCTestCase {
    private let app = XCUIApplication()

    private var serverURL: String {
        ProcessInfo.processInfo.environment["PHOENIX_UI_TEST_SERVER_URL"]
            ?? "https://127.0.0.1:8028"
    }

    private var workingDirectory: String {
        ProcessInfo.processInfo.environment["PHOENIX_UI_TEST_CWD"] ?? "/tmp"
    }

    override func setUpWithError() throws {
        continueAfterFailure = false
    }

    func testTLSMessageReconciliationAndColdRelaunch() throws {
        let runID = UUID().uuidString.prefix(8).lowercased()
        let seedMessage = "ios-ui-qa-\(runID) [[scenario:plain_text]]"
        let followUp = "canonical-\(runID) [[scenario:plain_text]]"

        app.launchArguments = ["-ui-testing-reset"]
        app.launch()

        let serverField = element("setup.serverURL")
        XCTAssertTrue(serverField.waitForExistence(timeout: 10))
        XCTAssertTrue(serverURL.hasPrefix("https://"))
        serverField.tap()
        serverField.typeText(String(serverURL.dropFirst("https://".count)))
        element("setup.connect").tap()

        XCTAssertTrue(app.navigationBars["Conversations"].waitForExistence(timeout: 20))
        attachScreenshot(named: "01-connected")

        element("conversationList.new").tap()
        let cwdField = element("newConversation.cwd")
        XCTAssertTrue(cwdField.waitForExistence(timeout: 10))
        replaceText(in: cwdField, with: workingDirectory)

        let modelPicker = element("newConversation.model")
        XCTAssertTrue(modelPicker.waitForExistence(timeout: 10))
        modelPicker.tap()
        let mockOption = app.buttons["mock"].firstMatch
        XCTAssertTrue(mockOption.waitForExistence(timeout: 10))
        mockOption.tap()

        let firstMessage = element("newConversation.message")
        focusMultilineField(firstMessage)
        firstMessage.typeText(seedMessage)

        let create = element("newConversation.create")
        waitUntilEnabled(create, timeout: 15)
        create.tap()

        let rowQuery = app.buttons.matching(
            NSPredicate(format: "identifier BEGINSWITH %@", "conversationList.row."))
        let createdRow = rowQuery.firstMatch
        XCTAssertTrue(createdRow.waitForExistence(timeout: 20))
        let createdRowIdentifier = createdRow.identifier
        createdRow.tap()

        XCTAssertTrue(message(identifier: "message.user", containing: seedMessage)
            .waitForExistence(timeout: 20))
        XCTAssertTrue(message(identifier: "message.agent", containing: "I've analyzed")
            .waitForExistence(timeout: 30))

        let composer = element("conversation.composer")
        focusMultilineField(composer)
        composer.typeText(followUp)
        let send = element("conversation.send")
        waitUntilEnabled(send, timeout: 10)
        send.tap()

        let authoritative = message(identifier: "message.user", containing: followUp)
        let optimistic = message(identifier: "message.outbox", containing: followUp)
        XCTAssertTrue(authoritative.waitForExistence(timeout: 20))
        waitUntilAbsent(optimistic, timeout: 20)
        XCTAssertEqual(matchingMessages(identifier: "message.user", containing: followUp).count, 1)
        XCTAssertEqual(matchingMessages(identifier: "message.outbox", containing: followUp).count, 0)
        attachScreenshot(named: "02-reconciled")

        app.terminate()
        app.launchArguments = []
        app.launch()

        XCTAssertTrue(app.navigationBars["Conversations"].waitForExistence(timeout: 15))
        let persistedRow = app.buttons[createdRowIdentifier]
        XCTAssertTrue(persistedRow.waitForExistence(timeout: 15))
        persistedRow.tap()

        XCTAssertTrue(message(identifier: "message.user", containing: followUp)
            .waitForExistence(timeout: 20))
        XCTAssertEqual(matchingMessages(identifier: "message.user", containing: followUp).count, 1)
        XCTAssertEqual(matchingMessages(identifier: "message.outbox", containing: followUp).count, 0)
        attachScreenshot(named: "03-cold-relaunch")
    }

    private func element(_ identifier: String) -> XCUIElement {
        app.descendants(matching: .any).matching(identifier: identifier).firstMatch
    }

    private func matchingMessages(identifier: String, containing text: String) -> XCUIElementQuery {
        app.descendants(matching: .any).matching(
            NSPredicate(
                format: "identifier == %@ AND label CONTAINS %@",
                identifier,
                text))
    }

    private func message(identifier: String, containing text: String) -> XCUIElement {
        matchingMessages(identifier: identifier, containing: text).firstMatch
    }

    private func replaceText(in field: XCUIElement, with text: String) {
        field.tap()
        field.typeKey("a", modifierFlags: .command)
        field.typeKey(.delete, modifierFlags: [])
        field.typeText(text)
    }

    private func focusMultilineField(_ field: XCUIElement) {
        // SwiftUI's axis-based TextField exposes its internal scroll view at
        // the element center. Tap the upper editable region instead.
        field.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2)).tap()
    }

    private func waitUntilEnabled(_ element: XCUIElement, timeout: TimeInterval) {
        let ready = NSPredicate(format: "exists == true AND enabled == true")
        let expectation = expectation(for: ready, evaluatedWith: element)
        XCTAssertEqual(XCTWaiter.wait(for: [expectation], timeout: timeout), .completed)
    }

    private func waitUntilAbsent(_ element: XCUIElement, timeout: TimeInterval) {
        let absent = NSPredicate(format: "exists == false")
        let expectation = expectation(for: absent, evaluatedWith: element)
        XCTAssertEqual(XCTWaiter.wait(for: [expectation], timeout: timeout), .completed)
    }

    private func attachScreenshot(named name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }
}
