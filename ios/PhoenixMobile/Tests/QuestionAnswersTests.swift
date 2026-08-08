import XCTest

@testable import PhoenixMobile

// Contract tests for question-answer encoding (REQ-IOS-016). The contract
// is the web QuestionPanel's answer map: keyed by question text;
// single-select = chosen label or free Other text; multi-select = labels
// joined with ", " plus trimmed Other text; nil until every question has
// an answer.
final class QuestionAnswersTests: XCTestCase {

    private let single = UserQuestion(
        question: "Which db?", header: "DB",
        options: [
            .init(label: "sqlite", description: ""),
            .init(label: "postgres", description: ""),
        ],
        multiSelect: false)

    private let multi = UserQuestion(
        question: "Which features?", header: "Feat",
        options: [
            .init(label: "auth", description: ""),
            .init(label: "sync", description: ""),
            .init(label: "push", description: ""),
        ],
        multiSelect: true)

    func testSingleSelectEncodesChosenLabel() {
        let answers = QuestionAnswers.encode(
            questions: [single],
            selections: ["Which db?": ["sqlite"]],
            otherTexts: [:])
        XCTAssertEqual(answers, ["Which db?": "sqlite"])
    }

    func testSingleSelectOtherTextUsedWhenNoOptionChosen() {
        let answers = QuestionAnswers.encode(
            questions: [single],
            selections: [:],
            otherTexts: ["Which db?": "  duckdb  "])
        XCTAssertEqual(answers, ["Which db?": "duckdb"], "Other text is trimmed")
    }

    func testMultiSelectJoinsLabelsInOptionOrder() {
        // Selection is a set; the declared option order defines the output
        // order, matching the panel's deterministic rendering.
        let answers = QuestionAnswers.encode(
            questions: [multi],
            selections: ["Which features?": ["push", "auth"]],
            otherTexts: [:])
        XCTAssertEqual(answers, ["Which features?": "auth, push"])
    }

    func testMultiSelectAppendsOtherText() {
        let answers = QuestionAnswers.encode(
            questions: [multi],
            selections: ["Which features?": ["sync"]],
            otherTexts: ["Which features?": "offline mode"])
        XCTAssertEqual(answers, ["Which features?": "sync, offline mode"])
    }

    func testMultiSelectOtherAloneIsSufficient() {
        let answers = QuestionAnswers.encode(
            questions: [multi],
            selections: [:],
            otherTexts: ["Which features?": "just this"])
        XCTAssertEqual(answers, ["Which features?": "just this"])
    }

    func testNilUntilEveryQuestionAnswered() {
        XCTAssertNil(
            QuestionAnswers.encode(
                questions: [single, multi],
                selections: ["Which db?": ["sqlite"]],
                otherTexts: [:]),
            "second question unanswered")
        XCTAssertNil(
            QuestionAnswers.encode(
                questions: [single],
                selections: [:],
                otherTexts: ["Which db?": "   "]),
            "whitespace-only Other is not an answer")
    }

    func testSelectionsForUnknownLabelsAreIgnored() {
        // A stale selection (e.g. from a superseded question payload) must
        // not fabricate an answer the current options don't contain.
        XCTAssertNil(
            QuestionAnswers.encode(
                questions: [single],
                selections: ["Which db?": ["mysql"]],
                otherTexts: [:]))
    }
}
