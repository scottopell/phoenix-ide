import XCTest

@testable import PhoenixMobile

// Contract tests for the SSE frame parser — the testing pattern for this
// app (see ios/README.md "Testing"): pure components get contract tests,
// one test per rule of the contract they implement; views stay untested.
// The contract here is the SSE wire format as the Phoenix server emits it
// (named events, JSON data lines, comment keep-alives, blank-line frame
// boundaries).
final class SSEParserTests: XCTestCase {

    /// Feed a raw stream through the byte-level parser, collecting frames.
    private func frames(from raw: String) -> [SSEFrame] {
        var parser = SSEParser()
        var out: [SSEFrame] = []
        for byte in Array(raw.utf8) {
            if let frame = parser.consume(byte) {
                out.append(frame)
            }
        }
        return out
    }

    func testNamedEventWithDataDispatchesOnBlankLine() {
        let out = frames(from: "event: message\ndata: {\"a\":1}\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].event, "message")
        XCTAssertEqual(out[0].data, "{\"a\":1}")
    }

    func testNoFrameEmittedBeforeBlankLine() {
        let out = frames(from: "event: message\ndata: {}\n")
        XCTAssertTrue(out.isEmpty, "frame must not dispatch until the blank line")
    }

    func testEventNameDefaultsToMessage() {
        let out = frames(from: "data: hello\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].event, "message")
        XCTAssertEqual(out[0].data, "hello")
    }

    func testMultiLineDataJoinedWithNewline() {
        let out = frames(from: "event: token\ndata: line1\ndata: line2\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].data, "line1\nline2")
    }

    func testCommentKeepAlivesProduceNoFrames() {
        // The server sends `: keep-alive` comments during quiet periods;
        // they must neither surface as frames nor break a following frame.
        let out = frames(from: ": keep-alive\n\n: ping\nevent: agent_done\ndata: {}\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].event, "agent_done")
    }

    func testCarriageReturnsStripped() {
        let out = frames(from: "event: message\r\ndata: x\r\n\r\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].event, "message")
        XCTAssertEqual(out[0].data, "x")
    }

    func testOnlyOneLeadingSpaceStrippedFromValue() {
        // Spec: a single space after the colon is part of the delimiter;
        // further whitespace belongs to the value.
        let out = frames(from: "data:  two spaces\n\n")
        XCTAssertEqual(out[0].data, " two spaces")
    }

    func testValueMayContainColons() {
        let out = frames(from: "data: {\"url\":\"https://x\"}\n\n")
        XCTAssertEqual(out[0].data, "{\"url\":\"https://x\"}")
    }

    func testNoSpaceAfterColonAccepted() {
        let out = frames(from: "data:tight\n\n")
        XCTAssertEqual(out[0].data, "tight")
    }

    func testBackToBackFrames() {
        let out = frames(from: "event: a\ndata: 1\n\nevent: b\ndata: 2\n\n")
        XCTAssertEqual(out.map(\.event), ["a", "b"])
        XCTAssertEqual(out.map(\.data), ["1", "2"])
    }

    func testBlankLinesWithoutAccumulatedFieldsEmitNothing() {
        let out = frames(from: "\n\n\n")
        XCTAssertTrue(out.isEmpty)
    }

    func testEventNameResetsBetweenFrames() {
        // A frame with only a name set (no data) still dispatches, and the
        // name must not leak into the next frame.
        let out = frames(from: "event: named\n\ndata: x\n\n")
        XCTAssertEqual(out.count, 2)
        XCTAssertEqual(out[0].event, "named")
        XCTAssertEqual(out[0].data, "")
        XCTAssertEqual(out[1].event, "message")
        XCTAssertEqual(out[1].data, "x")
    }

    func testUnknownFieldsIgnored() {
        let out = frames(from: "id: 42\nretry: 1000\nevent: message\ndata: x\n\n")
        XCTAssertEqual(out.count, 1)
        XCTAssertEqual(out[0].data, "x")
    }

    func testHardDeleteEventCarriesConversationIdentity() {
        let frame = SSEFrame(
            event: "conversation_hard_deleted",
            data: "{\"type\":\"conversation_hard_deleted\",\"sequence_id\":42,\"conversation_id\":\"c1\"}")
        guard case .conversationHardDeleted(let seq, let conversationId) =
            PhoenixEvent.decode(frame: frame)
        else {
            return XCTFail("expected typed hard-delete event")
        }
        XCTAssertEqual(seq, 42)
        XCTAssertEqual(conversationId, "c1")
    }
}
