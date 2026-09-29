import assert from "node:assert/strict";
import test from "node:test";

import {
  ACTIVATION_MARKER,
  PROJECTION_END,
  PROJECTION_START,
  parseRecordComment,
  reduceComments,
  renderRoadmap,
  run,
  surfaceDelivery,
  validateRecord,
  validateRecordText,
} from "./roadmap-issue-reducer.mjs";

const COORDINATOR = { role: "coordinator", harness: "phoenix@primary" };
const WORKER = { role: "worker", harness: "phoenix@devmbp" };
const CODEX = { role: "worker", harness: "codex" };
const USER = { role: "user", harness: "claude-code" };
const HEAD_A = "a".repeat(40);
const HEAD_B = "b".repeat(40);
const MERGE = "c".repeat(40);
const NOW = new Date("2026-09-27T12:00:00Z");

function body(record) {
  return `\`\`\`phoenix-roadmap\n${JSON.stringify({ version: 2, ...record }, null, 2)}\n\`\`\``;
}

function comment(id, record, overrides = {}) {
  const created = new Date(Date.UTC(2026, 8, 27, 0, 0, id)).toISOString();
  return {
    id,
    body: typeof record === "string" ? record : body(record),
    html_url: `https://github.test/issues/1#issuecomment-${id}`,
    created_at: created,
    updated_at: created,
    author_association: "OWNER",
    ...overrides,
  };
}

function outcome(overrides = {}) {
  return {
    kind: "outcome",
    actor: COORDINATOR,
    id: "ios-core-journeys",
    title: "Native iOS core journeys",
    intent: "Auth, send/stream, reconnect and safe delete work on device.",
    acceptance: ["Critical device journeys pass on a TestFlight build"],
    owner: { harness: "phoenix@devmbp", label: "iOS owner" },
    order: 10,
    surfaces: ["ios"],
    ...overrides,
  };
}

function qualified(pr, head, result = "pass", overrides = {}) {
  return {
    kind: "evidence",
    actor: WORKER,
    outcome: "ios-core-journeys",
    surface: "ios",
    stage: "qualified",
    result,
    subject: { pr, head },
    url: `https://github.com/o/r/pull/${pr}/checks`,
    ...overrides,
  };
}

function reduce(records) {
  return reduceComments(records.map((record, index) => comment(index + 1, record)));
}

function reasons(state) {
  return state.rejections.map((rejection) => rejection.reason);
}

test("only a comment consisting of one v2 fence is a record", () => {
  assert.deepEqual(parseRecordComment("hello"), null);
  assert.ok(parseRecordComment(body(outcome())).payload);
  assert.match(parseRecordComment("```phoenix-roadmap-update\n{}\n```").error, /v1 roadmap records/);
  assert.match(parseRecordComment(`note\n${body(outcome())}`).error, /only of one/);
});

test("records are validated structurally", () => {
  assert.throws(() => validateRecord({ version: 1, kind: "outcome", actor: COORDINATOR }), /version must be 2/);
  assert.throws(() => validateRecord({ version: 2, kind: "workstream", actor: COORDINATOR }), /unknown kind/);
  assert.throws(() => validateRecord({ version: 2, ...outcome({ extra: true }) }), /unknown field extra/);
  assert.throws(() => validateRecord({ version: 2, ...outcome({ id: "Not Kebab" }) }), /kebab-case/);
  assert.throws(() => validateRecord({ version: 2, ...qualified(5, "nothex") }), /commit SHA/);
  assert.throws(
    () => validateRecord({ version: 2, ...qualified(5, HEAD_A, "pass", { url: "http://devmbp:8031/c/x" }) }),
    /must use https/,
  );
});

test("roles bound which record kinds an actor may post", () => {
  const state = reduce([
    outcome({ actor: WORKER }),
    outcome(),
    { kind: "decision", actor: USER, id: "d-1", statement: "Ship it", scope: ["ios-core-journeys"] },
    { kind: "decision", actor: WORKER, id: "d-2", statement: "Ship it", scope: ["ios-core-journeys"] },
    { kind: "evidence", actor: WORKER, outcome: "ios-core-journeys", surface: "ios", stage: "accepted", result: "pass", subject: {}, url: "https://github.com/o/r" },
  ]);
  assert.deepEqual(reasons(state), [
    "worker may not post outcome records",
    "user decisions must quote the user's words",
    "worker may not post decision records",
    "accepted evidence comes from the user or the coordinator",
  ]);
});

test("coordinator records must come from the configured coordinator harness", () => {
  const state = reduceComments(
    [comment(1, outcome({ actor: { role: "coordinator", harness: "codex" } }))],
    { coordinatorHarness: "phoenix@primary" },
  );
  assert.deepEqual(reasons(state), ["coordinator records must come from phoenix@primary"]);
});

test("edited records, untrusted authors, and v1 records do not apply", () => {
  const state = reduceComments([
    comment(1, outcome(), { updated_at: "2026-09-28T00:00:00Z" }),
    comment(2, outcome({ id: "other" }), { author_association: "NONE" }),
    comment(3, "```phoenix-roadmap-update\n{}\n```"),
  ]);
  assert.equal(state.outcomes.size, 0);
  assert.deepEqual(state.rejections.map((rejection) => rejection.id), [1, 3]);
});

test("ids are unique across kinds and references must already exist", () => {
  const state = reduce([
    outcome(),
    { kind: "gate", actor: COORDINATOR, id: "ios-core-journeys", blocks: { outcome: "ios-core-journeys" }, clearer: "owner", condition: "x" },
    { kind: "gate", actor: COORDINATOR, id: "g-1", blocks: { outcome: "missing" }, clearer: "owner", condition: "x" },
    { kind: "status", actor: WORKER, outcome: "missing", next: "x" },
  ]);
  assert.deepEqual(reasons(state), [
    "id ios-core-journeys is already used; gate ids are single-use",
    "outcome missing does not exist",
    "outcome missing does not exist",
  ]);
});

// Scenario: a hold the user cleared must not resurrect.
test("a user-cleared gate clears only through a user decision and never reopens", () => {
  const gate = { kind: "gate", actor: COORDINATOR, id: "rc-env-secrets", blocks: { outcome: "ios-core-journeys" }, clearer: "user", condition: "Signing environment configured" };
  const state = reduce([
    outcome(),
    gate,
    { kind: "gate-clear", actor: COORDINATOR, gate: "rc-env-secrets", evidence: "https://github.com/o/r/settings" },
    { kind: "decision", actor: COORDINATOR, id: "d-env", statement: "Env done", scope: ["rc-env-secrets"], clears: ["rc-env-secrets"] },
    { kind: "decision", actor: USER, id: "d-env-user", statement: "Env done", scope: ["rc-env-secrets"], clears: ["rc-env-secrets"], quote: "setup is complete" },
    { ...gate, actor: WORKER },
  ]);
  assert.deepEqual(reasons(state), [
    "gate rc-env-secrets is cleared only by a user decision",
    "gate rc-env-secrets is cleared only by a user decision",
    "id rc-env-secrets is already used; gate ids are single-use",
  ]);
  assert.equal(state.gates.get("rc-env-secrets").cleared.decision, "d-env-user");
  assert.doesNotMatch(renderRoadmap(state, { now: NOW }), /Signing environment configured/);
});

test("owner gates clear with evidence; coordinator gates only by the coordinator", () => {
  const state = reduce([
    outcome(),
    { kind: "gate", actor: WORKER, id: "g-review", blocks: { outcome: "ios-core-journeys" }, clearer: "owner", condition: "Review on head" },
    { kind: "gate", actor: WORKER, id: "g-scope", blocks: { outcome: "ios-core-journeys" }, clearer: "coordinator", condition: "Scope" },
    { kind: "gate-clear", actor: WORKER, gate: "g-review", evidence: "https://github.com/o/r/pull/5#pullrequestreview-1" },
    { kind: "gate-clear", actor: WORKER, gate: "g-scope", evidence: "https://github.com/o/r" },
    { kind: "gate-clear", actor: WORKER, gate: "g-review", evidence: "https://github.com/o/r" },
  ]);
  assert.deepEqual(reasons(state), ["gate g-scope is cleared only by the coordinator", "gate g-review is already cleared"]);
});

// Scenario: a changed PR head invalidates old qualification without adopting late results.
test("qualification binds to the PR's current head", () => {
  const state = reduce([
    outcome(),
    qualified(5, HEAD_A),
    qualified(5, HEAD_B, "fail"),
    qualified(5, HEAD_A, "fail"),
  ]);
  const atB = surfaceDelivery(state, "ios-core-journeys", "ios", new Map([[5, { head: HEAD_B, merged: false, state: "open" }]]));
  assert.equal(atB.stage, "implemented");
  assert.deepEqual(atB.notes, ["✗ #5 qualification failed at bbbbbbb"]);

  const withPass = reduce([outcome(), qualified(5, HEAD_B), qualified(5, HEAD_A)]);
  const current = surfaceDelivery(withPass, "ios-core-journeys", "ios", new Map([[5, { head: HEAD_B, merged: false, state: "open" }]]));
  assert.equal(current.stage, "qualified");
  assert.equal(current.detail, "#5@bbbbbbb");
  assert.deepEqual(current.notes, ["#5 qualified at old head aaaaaaa; now bbbbbbb"]);
});

test("unverified PR state never counts as qualified", () => {
  const state = reduce([outcome(), qualified(5, HEAD_A)]);
  const delivery = surfaceDelivery(state, "ios-core-journeys", "ios", new Map());
  assert.equal(delivery.stage, "implemented");
  assert.deepEqual(delivery.notes, ["#5 state unverified"]);
});

// Scenario: a PR closed without merge (like #794) must not read as ready.
test("closed-without-merge PRs are history, not readiness", () => {
  const milestone = { kind: "milestone", actor: COORDINATOR, id: "m-1", title: "M", required: [{ outcome: "ios-core-journeys", surface: "ios", stage: "qualified" }] };
  const state = reduce([outcome(), milestone, qualified(794, HEAD_A)]);
  const closed = new Map([[794, { head: HEAD_A, merged: false, state: "closed" }]]);
  const delivery = surfaceDelivery(state, "ios-core-journeys", "ios", closed);
  assert.equal(delivery.stage, undefined);
  assert.deepEqual(delivery.notes, ["#794 closed without merge"]);
  const rendered = renderRoadmap(state, { pulls: closed, now: NOW });
  assert.match(rendered, /0\/1 required met/);
  assert.doesNotMatch(rendered, /All requirements met/);
  assert.match(renderRoadmap(state, { pulls: new Map(), now: NOW }), /0\/1 required met/);
});

test("closing one required component leaves the surface unsatisfied until explicitly resolved", () => {
  const implemented = (pr) => ({ kind: "evidence", actor: WORKER, outcome: "ios-core-journeys", surface: "ios", stage: "implemented", result: "pass", subject: { pr }, url: `https://github.com/o/r/pull/${pr}` });
  const milestone = { kind: "milestone", actor: COORDINATOR, id: "m-1", title: "M", required: [{ outcome: "ios-core-journeys", surface: "ios", stage: "merged" }] };
  const pulls = new Map([
    [10, { head: HEAD_A, merged: true, mergeCommit: MERGE, state: "closed" }],
    [11, { head: HEAD_B, merged: false, state: "closed" }],
    [12, { head: HEAD_B, merged: false, state: "open" }],
  ]);
  const base = [outcome(), milestone, implemented(10), implemented(11)];

  const closed = reduce(base);
  const delivery = surfaceDelivery(closed, "ios-core-journeys", "ios", pulls);
  assert.equal(delivery.stage, undefined);
  assert.deepEqual(delivery.unresolved, ["#11"]);
  const rendered = renderRoadmap(closed, { pulls, now: NOW });
  assert.match(rendered, /0\/1 required met/);
  assert.doesNotMatch(rendered, /All requirements met/);
  assert.match(rendered, /PR #11 closed without merge; withdraw or replace it/);

  const withdraw = { kind: "decision", actor: COORDINATOR, id: "d-drop-11", statement: "#11 is out of scope", scope: ["ios-core-journeys"], withdraws: [{ outcome: "ios-core-journeys", pr: 11 }] };
  const withdrawn = reduce([...base, withdraw]);
  const afterWithdrawal = surfaceDelivery(withdrawn, "ios-core-journeys", "ios", pulls);
  assert.equal(afterWithdrawal.stage, "merged");
  assert.deepEqual(afterWithdrawal.notes, ["#11 withdrawn by d-drop-11"]);
  assert.doesNotMatch(renderRoadmap(withdrawn, { pulls, now: NOW }), /closed without merge; withdraw/);

  const replaced = reduce([...base, withdraw, implemented(12)]);
  assert.equal(surfaceDelivery(replaced, "ios-core-journeys", "ios", pulls).stage, "implemented");

  const receipt = "https://github.com/o/r/issues/1#issuecomment-9";
  const whole = reduce([...base, { kind: "evidence", actor: WORKER, outcome: "ios-core-journeys", surface: "ios", stage: "deployed", result: "pass", subject: { target: "prod@primary", commit: MERGE }, url: receipt }]);
  const asserted = surfaceDelivery(whole, "ios-core-journeys", "ios", pulls);
  assert.equal(asserted.stage, "deployed");
  assert.deepEqual(asserted.unresolved, []);

  const outOfScope = reduce([...base, outcome({ id: "other", title: "Other" }), { ...withdraw, scope: ["other"] }]);
  assert.deepEqual(reasons(outOfScope), ["withdrawn outcome ios-core-journeys is outside the decision's scope"]);
});

test("merge is derived even when the latest qualification failed or is stale", () => {
  const state = reduce([outcome(), qualified(5, HEAD_A), qualified(5, HEAD_A, "fail")]);
  const merged = new Map([[5, { head: HEAD_B, merged: true, mergeCommit: MERGE, state: "closed" }]]);
  const delivery = surfaceDelivery(state, "ios-core-journeys", "ios", merged);
  assert.equal(delivery.stage, "merged");
  assert.equal(delivery.detail, "#5→ccccccc");
});

test("one merged component PR does not deliver a multi-part surface", () => {
  const implemented = (pr) => ({ kind: "evidence", actor: WORKER, outcome: "ios-core-journeys", surface: "ios", stage: "implemented", result: "pass", subject: { pr }, url: `https://github.com/o/r/pull/${pr}` });
  const state = reduce([outcome(), implemented(796), implemented(798)]);
  const pulls = new Map([
    [796, { head: HEAD_A, merged: false, state: "open" }],
    [798, { head: HEAD_B, merged: true, mergeCommit: MERGE, state: "closed" }],
  ]);
  const partial = surfaceDelivery(state, "ios-core-journeys", "ios", pulls);
  assert.equal(partial.stage, "implemented");
  assert.equal(partial.detail, "#796");
  assert.deepEqual(partial.notes, ["#798→ccccccc merged"]);

  pulls.set(796, { head: HEAD_A, merged: true, mergeCommit: MERGE, state: "closed" });
  assert.equal(surfaceDelivery(state, "ios-core-journeys", "ios", pulls).stage, "merged");
});

test("whole-surface delivery keeps its identity and shows open follow-ups", () => {
  const receipt = "https://github.com/o/r/issues/1#issuecomment-9";
  const deployed = (commit) => ({ kind: "evidence", actor: WORKER, outcome: "ios-core-journeys", surface: "ios", stage: "deployed", result: "pass", subject: { target: "prod@primary", commit }, url: receipt });
  const state = reduce([
    outcome(),
    deployed(HEAD_A),
    deployed(HEAD_B),
    { kind: "evidence", actor: WORKER, outcome: "ios-core-journeys", surface: "ios", stage: "implemented", result: "pass", subject: { pr: 9 }, url: "https://github.com/o/r/pull/9" },
  ]);
  const delivery = surfaceDelivery(state, "ios-core-journeys", "ios", new Map([[9, { head: HEAD_A, merged: false, state: "open" }]]));
  assert.equal(delivery.stage, "deployed");
  assert.equal(delivery.detail, "prod@primary@bbbbbbb");
  assert.deepEqual(delivery.notes, ["open follow-up #9 (implemented)"]);
});

test("pass and fail for the same head needs the coordinator", () => {
  const state = reduce([outcome(), qualified(5, HEAD_A), qualified(5, HEAD_A, "fail"), qualified(5, HEAD_A)]);
  assert.equal(state.conflicts.length, 1);
  assert.match(state.conflicts[0].message, /both pass and fail/);
});

test("merge state is derived from GitHub", () => {
  const state = reduce([outcome(), qualified(5, HEAD_A)]);
  const delivery = surfaceDelivery(state, "ios-core-journeys", "ios", new Map([[5, { head: HEAD_A, merged: true, mergeCommit: MERGE }]]));
  assert.equal(delivery.stage, "merged");
  assert.equal(delivery.detail, "#5→ccccccc");
});

// Scenario: continued ownership survives transcript continuation; other harnesses are flagged.
test("status replaces execution pointers and flags non-owner harnesses", () => {
  const state = reduce([
    outcome(),
    { kind: "status", actor: WORKER, outcome: "ios-core-journeys", next: "Qualify", pointers: [{ label: "old transcript", url: "http://devmbp:8031/c/old", harness: "phoenix@devmbp" }] },
    { kind: "status", actor: WORKER, outcome: "ios-core-journeys", next: "Build for TestFlight", pointers: [{ label: "continuation", url: "http://devmbp:8031/c/new", harness: "phoenix@devmbp" }] },
    { kind: "status", actor: CODEX, outcome: "ios-core-journeys", next: "I own this now" },
  ]);
  assert.equal(state.outcomes.get("ios-core-journeys").owner.harness, "phoenix@devmbp");
  assert.equal(state.conflicts.length, 1);
  assert.match(state.conflicts[0].message, /status from codex, but the accountable owner is phoenix@devmbp/);
  const rendered = renderRoadmap(state, { now: NOW });
  assert.doesNotMatch(rendered, /old transcript/);
});

// Scenario: web and native delivery coexist without flattening.
test("each surface reports its own delivery stage", () => {
  const state = reduce([
    outcome({ id: "auto-continue", title: "Auto-continuation", surfaces: ["web", "ios"] }),
    { kind: "evidence", actor: WORKER, outcome: "auto-continue", surface: "web", stage: "deployed", result: "pass", subject: { target: "prod@devmbp", commit: "87606f42404d8d169b85cea2f6de3e6732a3e58f" }, url: "https://github.com/o/r/issues/1#issuecomment-2" },
  ]);
  assert.equal(surfaceDelivery(state, "auto-continue", "web", new Map()).detail, "prod@devmbp@87606f4");
  assert.equal(surfaceDelivery(state, "auto-continue", "ios", new Map()).stage, undefined);
  assert.match(renderRoadmap(state, { now: NOW }), /web: deployed prod@devmbp@87606f4<br>ios: —/);
});

// Scenario: an optional outcome's gates never block its milestone.
test("optional outcomes do not block and requirement changes need a decision", () => {
  const milestone = {
    kind: "milestone",
    actor: COORDINATOR,
    id: "testflight-1",
    title: "Native iOS on TestFlight",
    required: [{ outcome: "ios-core-journeys", surface: "ios", stage: "released" }],
    optional: ["native-auto-continue"],
  };
  const state = reduce([
    outcome(),
    outcome({ id: "native-auto-continue", title: "Native auto-continue controls", order: 50 }),
    milestone,
    { kind: "gate", actor: WORKER, id: "g-parity", blocks: { outcome: "native-auto-continue" }, clearer: "owner", condition: "Parity design approved" },
    { ...milestone, required: [...milestone.required, { outcome: "native-auto-continue", surface: "ios", stage: "released" }], optional: [] },
    { kind: "decision", actor: USER, id: "d-tf-scope", statement: "Auto-continue is not a TestFlight prerequisite", scope: ["testflight-1"], quote: "not a TestFlight blocker" },
    { kind: "gate", actor: WORKER, id: "g-milestone", blocks: { milestone: "testflight-1" }, clearer: "owner", condition: "x" },
  ]);
  assert.deepEqual(reasons(state), [
    "changing milestone testflight-1 requirements needs a current decision scoped to it",
    "only the coordinator may gate a milestone",
  ]);
  const rendered = renderRoadmap(state, { now: NOW });
  const milestoneSection = rendered.slice(rendered.indexOf("## Milestones"), rendered.indexOf("## Outcomes outside milestones"));
  assert.match(milestoneSection, /0\/1 required met/);
  assert.match(milestoneSection, /Optional, not blocking: Native auto-continue controls/);
  assert.doesNotMatch(milestoneSection, /Parity design approved/);
  assert.match(milestoneSection, /d-tf-scope/);

  const superseding = reduce([
    outcome(),
    outcome({ id: "native-auto-continue", title: "Native auto-continue controls" }),
    milestone,
    { kind: "decision", actor: COORDINATOR, id: "d-hold", statement: "Hold for setup", scope: ["ios-core-journeys"] },
    { kind: "decision", actor: COORDINATOR, id: "d-hold-cleared", statement: "Setup complete", scope: ["ios-core-journeys"], supersedes: ["d-hold"] },
  ]);
  const decisionsLine = renderRoadmap(superseding, { now: NOW }).split("\n").find((text) => text.startsWith("Decisions:"));
  assert.match(decisionsLine, /d-hold-cleared/);
  assert.doesNotMatch(decisionsLine, /Hold for setup/);

  const changed = reduce([
    outcome(),
    outcome({ id: "native-auto-continue", title: "Native auto-continue controls" }),
    milestone,
    { kind: "decision", actor: USER, id: "d-add", statement: "Now required", scope: ["testflight-1"], quote: "make it required" },
    { ...milestone, required: [...milestone.required, { outcome: "native-auto-continue", surface: "ios", stage: "released" }], optional: [], decision: "d-add" },
  ]);
  assert.deepEqual(reasons(changed), []);
  assert.equal(changed.milestones.get("testflight-1").required.length, 2);
});

test("retired outcomes stay retired and dropped requirements need the coordinator", () => {
  const state = reduce([
    outcome(),
    { kind: "milestone", actor: COORDINATOR, id: "m-1", title: "M", required: [{ outcome: "ios-core-journeys", surface: "ios", stage: "merged" }] },
    { kind: "outcome-retire", actor: COORDINATOR, outcome: "ios-core-journeys", reason: "dropped", note: "Superseded" },
    qualified(5, HEAD_A),
    outcome(),
  ]);
  assert.deepEqual(reasons(state), ["outcome ios-core-journeys is retired", "outcome ios-core-journeys is retired"]);
  const rendered = renderRoadmap(state, { now: NOW });
  assert.match(rendered, /M requires dropped outcome ios-core-journeys/);
  assert.match(rendered, /## Retired in the last 14 days\n\n- Native iOS core journeys — dropped: Superseded/);
});

test("forked supersession is a coordinator conflict", () => {
  const state = reduce([
    outcome(),
    { kind: "decision", actor: COORDINATOR, id: "d-1", statement: "A", scope: ["ios-core-journeys"] },
    { kind: "decision", actor: COORDINATOR, id: "d-2", statement: "B", scope: ["ios-core-journeys"], supersedes: ["d-1"] },
    { kind: "decision", actor: COORDINATOR, id: "d-3", statement: "C", scope: ["ios-core-journeys"], supersedes: ["d-1"] },
  ]);
  assert.match(state.conflicts[0].message, /d-1 is superseded by both d-2 and d-3/);
});

test("rendering shows freshness and escapes table text", () => {
  const state = reduceComments([
    comment(1, outcome({ title: "Pipes | and *stars*" })),
    comment(2, { kind: "status", actor: WORKER, outcome: "ios-core-journeys", next: "Next | step" }),
    comment(3, "```phoenix-roadmap\nnot json\n```"),
  ]);
  const rendered = renderRoadmap(state, { now: NOW, snapshotThroughCommentId: 3 });
  assert.ok(rendered.includes(PROJECTION_START) && rendered.includes(PROJECTION_END) && rendered.includes(ACTIVATION_MARKER));
  assert.match(rendered, /snapshot-through:3/);
  assert.match(rendered, /Pipes \\\| and \\\*stars\\\*/);
  assert.match(rendered, /Next \\\| step/);
  assert.match(rendered, /\| 11h \|/);
  assert.match(rendered, /- \[3\]\(.*\): invalid JSON/);

  const stale = renderRoadmap(state, { now: new Date("2026-10-05T00:00:00Z") });
  assert.match(stale, /\| 7d ⚠ \|/);
});

function acknowledgement(rendered, commentId) {
  const snapshot = Number(rendered.match(/snapshot-through:(\d+)/)[1]);
  const windowFrom = Number(rendered.match(/ack-window-from:(\d+)/)[1]);
  const accepted = new Set(rendered.match(/phoenix-roadmap:accepted:([\d,]*)/)[1].split(",").filter(Boolean).map(Number));
  if (commentId > snapshot) return "pending";
  if (commentId < windowFrom) return "unknown";
  if (accepted.has(commentId)) return "accepted";
  return rendered.includes(`- [${commentId}](`) ? "rejected" : "unknown";
}

test("acknowledgement is unambiguous within a bounded window and unknown before it", () => {
  const comments = [comment(1, outcome())];
  for (let id = 2; id <= 22; id += 1) comments.push(comment(id, "```phoenix-roadmap\nnot json\n```"));
  const rendered = renderRoadmap(reduceComments(comments), { now: NOW, snapshotThroughCommentId: 22 });
  assert.equal(acknowledgement(rendered, 1), "accepted");
  assert.equal(acknowledgement(rendered, 2), "rejected");
  assert.equal(acknowledgement(rendered, 22), "rejected");
  assert.equal(acknowledgement(rendered, 23), "pending");

  for (let id = 23; id <= 80; id += 1) comments.push(comment(id, { kind: "status", actor: WORKER, outcome: "ios-core-journeys", next: `check-in ${id}` }));
  const later = renderRoadmap(reduceComments(comments), { now: NOW, snapshotThroughCommentId: 80 });
  assert.equal(acknowledgement(later, 2), "unknown");
  assert.equal(acknowledgement(later, 31), "accepted");
  assert.match(later, /ack-window-from:31/);
});

test("deleted records and comments edited out of record form are unknown, not accepted", () => {
  const status = { kind: "status", actor: WORKER, outcome: "ios-core-journeys", next: "x" };
  const deleted = renderRoadmap(reduceComments([comment(1, outcome()), comment(3, status)]), { now: NOW, snapshotThroughCommentId: 3 });
  assert.equal(acknowledgement(deleted, 1), "accepted");
  assert.equal(acknowledgement(deleted, 2), "unknown");
  assert.equal(acknowledgement(deleted, 3), "accepted");

  const defenced = comment(2, "never mind", { updated_at: "2026-09-28T00:00:00Z" });
  const rendered = renderRoadmap(reduceComments([comment(1, outcome()), defenced, comment(3, status)]), { now: NOW, snapshotThroughCommentId: 3 });
  assert.equal(acknowledgement(rendered, 2), "unknown");
});

test("a record edited after acceptance is reported as rejected on reprocessing", () => {
  const edited = comment(2, { kind: "status", actor: WORKER, outcome: "ios-core-journeys", next: "x" }, { updated_at: "2026-09-28T00:00:00Z" });
  const rendered = renderRoadmap(reduceComments([comment(1, outcome()), edited]), { now: NOW, snapshotThroughCommentId: 2 });
  assert.equal(acknowledgement(rendered, 2), "rejected");
  assert.match(rendered, /edited records are ignored/);
});

test("decisions clear and supersede only within their scope", () => {
  const state = reduce([
    outcome(),
    outcome({ id: "other", title: "Other" }),
    { kind: "gate", actor: COORDINATOR, id: "g-1", blocks: { outcome: "ios-core-journeys" }, clearer: "coordinator", condition: "x" },
    { kind: "decision", actor: COORDINATOR, id: "d-other", statement: "x", scope: ["other"], clears: ["g-1"] },
    { kind: "decision", actor: COORDINATOR, id: "d-a", statement: "x", scope: ["ios-core-journeys"] },
    { kind: "decision", actor: COORDINATOR, id: "d-b", statement: "x", scope: ["other"], supersedes: ["d-a"] },
    { kind: "decision", actor: USER, id: "d-user", statement: "x", scope: ["ios-core-journeys"], clears: ["g-1"], quote: "clear it" },
  ]);
  assert.deepEqual(reasons(state), ["gate g-1 is outside the decision's scope", "decision d-a shares no scope with d-b"]);
  assert.equal(state.gates.get("g-1").cleared.decision, "d-user");
});

test("evidence must be GitHub-checkable and deployment claims need a receipt", () => {
  const deployed = (url) => ({ version: 2, kind: "evidence", actor: WORKER, outcome: "x", surface: "server", stage: "deployed", result: "pass", subject: { target: "prod@primary", commit: HEAD_A }, url });
  assert.throws(() => validateRecord(deployed("https://example.com/receipt")), /github.com URL/);
  assert.throws(() => validateRecord(deployed("https://github.com/o/r/commit/abc")), /GitHub receipt/);
  assert.equal(validateRecord(deployed("https://github.com/o/r/pull/807#issuecomment-5859803423")).stage, "deployed");
  assert.equal(validateRecord(deployed("https://github.com/o/r/actions/runs/123")).stage, "deployed");
});

test("length errors name the offending field", () => {
  const status = { version: 2, kind: "status", actor: WORKER, outcome: "x", next: "y".repeat(201) };
  assert.throws(() => validateRecord(status), /^Error: next must be at most 200 characters$|next must be at most 200 characters/);
  assert.doesNotThrow(() => validateRecord({ ...status, next: "y".repeat(200) }));
});

test("records can be validated standalone before posting", () => {
  assert.equal(validateRecordText(body(outcome())).ok, true);
  assert.equal(validateRecordText(JSON.stringify({ version: 2, ...outcome() })).ok, true);
  assert.match(validateRecordText("{").error, /invalid JSON/);
  assert.match(validateRecordText(JSON.stringify({ version: 2, ...outcome({ actor: WORKER }) })).error, /may not post outcome/);
});

const ACTIVE_BODY = `# Phoenix delivery roadmap\n\n${ACTIVATION_MARKER}\n`;

function fakeApi(comments, { pulls = {}, body = ACTIVE_BODY, failReactions = [], failReplace = false } = {}) {
  const calls = { reactions: [], bodies: [], pulls: [] };
  return {
    calls,
    getIssueBody: async () => body,
    listComments: async () => comments,
    replaceBody: async (next) => {
      if (failReplace) throw new Error("PATCH failed");
      calls.bodies.push(next);
    },
    getPull: async (number) => {
      calls.pulls.push(number);
      if (!pulls[number]) throw new Error("404");
      return pulls[number];
    },
    setReaction: async (id, content) => {
      if (failReactions.includes(content)) throw new Error(`${content} failed`);
      calls.reactions.push([id, content]);
    },
  };
}

function commentEvent(created) {
  return { action: "created", issue: { number: 7 }, comment: created };
}

const RUN = { configuredIssueNumber: 7, coordinatorHarness: "phoenix@primary", now: () => NOW };

test("run acknowledges an accepted record created on the roadmap Issue", async () => {
  const created = comment(2, qualified(5, HEAD_A));
  const api = fakeApi([comment(1, outcome()), created], { pulls: { 5: { head: HEAD_A, merged: false, state: "open" } } });
  const result = await run({ ...RUN, eventName: "issue_comment", event: commentEvent(created), api });
  assert.equal(result.acknowledged, "accepted");
  assert.deepEqual(api.calls.reactions, [[2, "eyes"], [2, "rocket"]]);
  assert.deepEqual(api.calls.pulls, [5]);
  assert.match(api.calls.bodies[0], /PR state verified at render/);
  assert.match(api.calls.bodies[0], /ios: qualified #5@aaaaaaa/);
});

test("run marks a rejected record confused", async () => {
  const created = comment(1, qualified(5, HEAD_A));
  const api = fakeApi([created]);
  const result = await run({ ...RUN, eventName: "issue_comment", event: commentEvent(created), api });
  assert.equal(result.acknowledged, "rejected");
  assert.deepEqual(api.calls.reactions.at(-1), [1, "confused"]);
});

test("reaction failures never block or contradict the projection", async () => {
  const created = comment(2, { kind: "status", actor: WORKER, outcome: "ios-core-journeys", next: "x" });
  const api = fakeApi([comment(1, outcome()), created], { failReactions: ["eyes", "rocket"] });
  const result = await run({ ...RUN, eventName: "issue_comment", event: commentEvent(created), api });
  assert.equal(result.acknowledged, "accepted");
  assert.equal(api.calls.bodies.length, 1);
  assert.deepEqual(api.calls.reactions, []);
});

test("a projection failure fails the run without marking the record rejected", async () => {
  const created = comment(2, { kind: "status", actor: WORKER, outcome: "ios-core-journeys", next: "x" });
  const api = fakeApi([comment(1, outcome()), created], { failReplace: true });
  await assert.rejects(run({ ...RUN, eventName: "issue_comment", event: commentEvent(created), api }), /PATCH failed/);
  assert.deepEqual(api.calls.reactions, [[2, "eyes"]]);
});

test("run skips other Issues and untrusted authors", async () => {
  const api = fakeApi([]);
  const created = comment(1, outcome());
  assert.deepEqual(
    await run({ ...RUN, eventName: "issue_comment", event: { ...commentEvent(created), issue: { number: 8 } }, api }),
    { skipped: "not the configured roadmap Issue" },
  );
  const untrusted = { ...created, author_association: "NONE" };
  assert.deepEqual(
    await run({ ...RUN, eventName: "issue_comment", event: commentEvent(untrusted), api }),
    { skipped: "triggering author is not trusted" },
  );
  assert.equal(api.calls.bodies.length, 0);
});

// Scenario: the reducer merges to main before the Issue variable is switched, or the switch is denied.
test("an unactivated Issue or missing coordinator binding is never written", async () => {
  const v1Body = "# Phoenix delivery roadmap\n\n<!-- phoenix-roadmap:projection:start -->\nv1 rows\n";
  const v1Comment = comment(1, "```phoenix-roadmap-update\n{}\n```");
  const onV1 = fakeApi([v1Comment], { body: v1Body });
  assert.match((await run({ ...RUN, eventName: "schedule", event: {}, api: onV1 })).skipped, /lacks the v2 activation marker/);
  assert.match((await run({ ...RUN, eventName: "issue_comment", event: commentEvent(v1Comment), api: onV1 })).skipped, /activation marker/);
  assert.deepEqual(onV1.calls.bodies, []);
  assert.deepEqual(onV1.calls.reactions, []);

  const unbound = fakeApi([comment(1, outcome())]);
  assert.match((await run({ ...RUN, coordinatorHarness: undefined, eventName: "workflow_dispatch", event: {}, api: unbound })).skipped, /no coordinator harness/);
  assert.deepEqual(unbound.calls.bodies, []);
});

test("scheduled runs re-render without acknowledging and tolerate PR read failures", async () => {
  const api = fakeApi([comment(1, outcome()), comment(2, qualified(5, HEAD_A))]);
  const result = await run({ ...RUN, eventName: "schedule", event: {}, api });
  assert.deepEqual(result, { outcomes: 1, rejections: 0 });
  assert.deepEqual(api.calls.reactions, []);
  assert.match(api.calls.bodies[0], /PR state not verified/);
  assert.match(api.calls.bodies[0], /#5 state unverified/);
});
