"""Offline guidance/source drift checks, not runtime or deployed-API acceptance."""

import json
import re
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
API = ROOT / "crates/phoenix-ide/src/api/handlers.rs"
REFERENCE = ROOT / "crates/phoenix-skills/src/builtin/phoenix-api/references/api-reference.md"


class PhoenixApiGuidanceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.handlers = API.read_text()
        cls.reference = REFERENCE.read_text()
        cls.recipe = cls.reference.split("## Respond to a pending structured question\n", 1)[1].split(
            "## Honest gaps", 1
        )[0]
        cls.handler = cls.handlers.split("async fn respond_to_question(", 1)[1].split(
            "async fn dismiss_question(", 1
        )[0]

    def test_question_json_matches_registered_route_and_payload(self):
        self.assertIn('"/api/conversations/:id/respond", post(respond_to_question)', self.handlers)
        self.assertIn("POST /api/conversations/{id}/respond", self.recipe)
        match = re.search(r"```json\s+(.*?)\s+```", self.recipe, re.S)
        self.assertIsNotNone(match)
        body = json.loads(match.group(1))
        self.assertEqual(set(body), {"request_id", "answers"})
        self.assertIsInstance(body["request_id"], str)
        self.assertTrue(body["answers"])
        self.assertTrue(all(isinstance(k, str) and isinstance(v, str) for k, v in body["answers"].items()))
        payload = self.handlers.split("struct RespondToQuestionPayload {", 1)[1].split(
            "struct DismissQuestionPayload", 1
        )[0]
        self.assertIn("request_id: Option<crate::state_machine::state::QuestionRequestId>", payload)
        self.assertIn("answers: std::collections::HashMap<String, String>", payload)
        ui = (ROOT / "ui/src/api.ts").read_text()
        self.assertIn("JSON.stringify({ request_id: requestId, answers, annotations })", ui)
        self.assertIn("GET /api/conversations/{id}", self.recipe)
        get = self.handlers.split("async fn get_conversation(", 1)[1].split(
            "fn validate_message_history_limit", 1
        )[0]
        self.assertIn("conversation: conversation_to_json_with_seed", get)

    def test_freshness_contract_matches_admission_locked_server_checks(self):
        guard = self.handler.index("let _admission_guard = admission.lock().await;")
        read = self.handler.index(".get_conversation(&id)")
        state = self.handler.index("ConvState::AwaitingUserResponse")
        identity = self.handler.index("validate_question_request_identity(")
        admission = self.handler.index("require_ordinary_mutation_admission(")
        send = self.handler.index(".send_event(")
        self.assertEqual(sorted([guard, read, state, identity, admission, send]),
                         [guard, read, state, identity, admission, send])
        validator = self.handlers.split("fn validate_question_request_identity(", 1)[1].split(
            "#[cfg(test)]", 1
        )[0]
        self.assertIn("if pending == submitted", validator)
        self.assertIn("AppError::Conflict", validator)
        core = (ROOT / "crates/phoenix-core/src/domain/sm_state.rs").read_text()
        self.assertIn('"type": "awaiting_user_response"', core)
        self.assertIn('rename = "request_id"', core)
        for text in ("conversation.state.request_id", "not `tool_use_id`",
                     "freshness guard, not an idempotency key", "POST once",
                     "do not automatically POST again", "not success"):
            self.assertIn(text, self.recipe)

    def test_answers_and_notes_match_transition_and_ui_not_extra_guarantees(self):
        transition = (ROOT / "crates/phoenix-state-machine/src/transition.rs").read_text()
        response = transition.split("let answers_text = questions", 1)[1].split(
            "let user_text =", 1
        )[0]
        self.assertIn(".filter_map(|q|", response)
        self.assertIn("answers.get(&q.question)?", response)
        self.assertIn("ann.notes", response)
        self.assertNotIn("ann.preview", response)
        self.assertIn(".find(|o| o.label == *a)", response)
        self.assertIn("o.preview.as_deref()", response)
        panel = (ROOT / "ui/src/components/QuestionPanel.tsx").read_text()
        self.assertIn("result[q.question] = labels.join(', ')", panel)
        for text in ("every exact full question text", "backend does not enforce answer completeness",
                     "one comma-space string", "Submitted annotation `preview` is not used",
                     "exact user-provided answers", "changed state alone does not prove"):
            self.assertIn(text, self.recipe)

    def test_exact_answers_and_explicit_delegation_are_both_permitted(self):
        paths = [
            REFERENCE.parent.parent / "SKILL.md",
            REFERENCE,
            ROOT / "specs/global-recall/requirements.md",
            ROOT / "specs/builtin-skills/builtin-skills.allium",
            ROOT / "specs/adrs/113_coordinator-api-recipes-are-non-exhaustive.md",
            ROOT / "specs/builtin-skills/executive.md",
            ROOT / "specs/global-recall/executive.md",
        ]
        for path in paths:
            with self.subTest(path=path):
                # Allium guidance wraps its prose in line comments.
                text = " ".join(path.read_text().replace("--", "").split()).lower()
                self.assertIn("exact user-provided answers or decisions made within explicit delegated decision scope", text)
                self.assertIn("decision authority", text)
                self.assertIn("approvals", text)
                self.assertIn("delegation", text)
                self.assertNotIn("obtain the user's decision for every question", text)
                self.assertNotIn("not authority for global to make that decision", text)
        # A positive delegated choice is documented along with its scope limit.
        self.assertIn("“choose the test runner for this task” permits choosing a runner for that task", self.recipe)
        self.assertIn("not approving deployment", self.recipe)
        self.assertIn("Never fabricate approvals or extend delegation", self.recipe)
        self.assertIn("ask the user when neither applies", self.recipe)
        self.assertIn("Reassess the new questions separately", self.recipe)

    def test_gap_inventory_does_not_deny_verified_alternatives_or_bounded_reads(self):
        gaps = self.reference.split("## Honest gaps", 1)[1]
        self.assertIn("illustrative inventory qualified by inspected deployment/source", gaps)
        self.assertIn("not a denylist", gaps)
        self.assertIn("Verified supported alternatives and bounded state reads remain available", gaps)
        self.assertIn("Bounded, deadline-limited observation through supported reads is permitted", gaps)
        for text in ("Never invent guarantees or receipts", "database writes or private handlers",
                     "or create background monitors", "missing server guarantee"):
            self.assertIn(text, gaps)
        self.assertNotIn("Current APIs do not provide", self.reference)
        self.assertIn("Use a verified supported bounded state read", self.reference)
        self.assertIn("Never infer state from a partial body", self.reference)
        self.assertNotIn("the public API does not expose", self.reference)
        self.assertNotIn("the public API has no bounded", self.reference)
        # The built-in regression must not lock the categorical inventory back in.
        builtin = (ROOT / "crates/phoenix-skills/src/builtin.rs").read_text()
        self.assertNotIn('assert!(reference.contains("Current APIs do not provide"))', builtin)

    def test_literal_recipe_requires_capability_and_dedicated_route(self):
        recipe = self.reference.split("## Capability-gated public literal chat\n", 1)[1].split(
            "## Respond to a pending structured question", 1
        )[0]
        self.assertIn('"/api/conversations/:id/chat/literal",', self.handlers)
        self.assertIn("post(send_literal_chat).layer(DefaultBodyLimit::max(MAX_MULTIPART_BODY_BYTES))", self.handlers)
        self.assertIn("POST /api/conversations/{original_transcript_id}/chat/literal", recipe)
        body = json.loads(re.search(r"```json\s+(.*?)\s+```", recipe, re.S).group(1))
        self.assertEqual(body["expansion_policy"], "literal_text")
        self.assertEqual(set(body), {"text", "message_id", "expansion_policy", "images", "files", "user_agent"})
        self.assertEqual(body["images"], [])
        self.assertEqual(body["files"], [])
        for text in ("`chat_literal_text_v1` field is boolean `true`", "stop before POST",
                     "no redirects", "never fall back to `/chat`", "same original destination/transcript",
                     "historical receiver", "not itself proof of owner authentication"):
            self.assertIn(text, recipe)
        self.assertLess(recipe.index("GET /api/version"), recipe.index("4. POST once"))
        skill = (REFERENCE.parent.parent / "SKILL.md").read_text()
        self.assertIn("/api/conversations/{original_transcript_id}/chat/literal", skill)
        self.assertIn("never fall back to `/chat`", skill)

    def test_literal_handler_validates_before_shared_admission_and_closed_policy(self):
        handler = self.handlers.split("async fn send_literal_chat(", 1)[1].split("async fn send_chat(", 1)[0]
        self.assertLess(handler.index("req.expansion_policy != ChatExpansionPolicy::LiteralText"),
                        handler.index("send_chat(state, id, Json(req)).await"))
        self.assertIn("AppError::BadRequest", handler)
        types = (ROOT / "crates/phoenix-ide/src/api/types.rs").read_text()
        policy = types.split("pub enum ChatExpansionPolicy {", 1)[1].split("}", 1)[0]
        self.assertEqual(re.findall(r"^\s+(\w+),$", policy, re.M), ["ExpandReferences", "LiteralText"])
        self.assertIn("#[default]\n    ExpandReferences", policy)
        self.assertIn('"chat_literal_text_v1": true', self.handlers)
        service = (ROOT / "crates/phoenix-ide/src/send_chat_service.rs").read_text()
        replay = service.split("async fn lookup_durable_steering_replay(", 1)[1].split(
            "async fn lookup_persisted_message_replay(", 1
        )[0]
        self.assertIn("} else if req.expansion_policy != MessageExpansionPolicy::ExpandReferences {", replay)
        self.assertIn("Err(SendChatServiceError::IdempotencyConflict)", replay)
        for path in ("specs/api/requirements.md", "specs/builtin-skills/builtin-skills.allium",
                     "specs/compatibility/requirements.md"):
            text = (ROOT / path).read_text()
            self.assertIn("chat_literal_text_v1", text)
            self.assertIn("/chat/literal", text)

    def test_non_exhaustive_policy_keeps_public_contract_and_scope_boundaries(self):
        skill = (REFERENCE.parent.parent / "SKILL.md").read_text()
        prompt = (ROOT / "crates/phoenix-ide/src/system_prompt.rs").read_text().split(
            "#[cfg(test)]", 1
        )[0]
        allium = (ROOT / "specs/builtin-skills/builtin-skills.allium").read_text()
        for content in (skill, self.reference, prompt):
            self.assertIn("non-exhaustive, not an endpoint allowlist", content)
            self.assertIn("current deployment/source contracts", content)
            self.assertNotIn("Inspect source only when", content)
            self.assertNotIn("Documented Phoenix API operations", content)
        self.assertNotIn("Source inspection is a version-mismatch diagnostic", allium)
        self.assertIn("active local WorkScope authorizes local execution only", skill)
        self.assertIn("user-identified credential source bound to the selected destination", self.reference)
        self.assertIn("Do not disable TLS verification", self.reference)
        self.assertIn("user-authorized, authenticated encrypted transport", self.reference)
        self.assertIn("Never write lifecycle state through SQLite", self.reference)
        self.assertIn("same original predecessor, message identity, and intent", self.reference)
        self.assertIn("does not authorize choosing its answers", self.recipe)


if __name__ == "__main__":
    unittest.main()
