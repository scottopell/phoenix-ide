import assert from "node:assert/strict";
import test from "node:test";

import {
  PROJECTION_END,
  PROJECTION_START,
  renderRoadmap,
  run,
  updatesFromComments,
  validateUpdate,
} from "./roadmap-issue-reducer.mjs";

function update(overrides = {}) {
  return {
    kind: "workstream-update",
    version: 1,
    workstream: "ios-vnext",
    title: "iOS vNext",
    state: "Blocked",
    owner: "iOS vNext",
    blocked_by: ["ProductConversation PR 3"],
    next: "ProductConversation migration",
    section: "blocked-programs",
    order: 20,
    evidence: [{ label: "#641", url: "https://github.com/scottopell/phoenix-ide/pull/641" }],
    context: "Use the stable aggregate contract.",
    ...overrides,
  };
}

function comment(id, body, overrides = {}) {
  return {
    id,
    body,
    html_url: `https://github.test/issues/1#issuecomment-${id}`,
    created_at: `2026-08-09T16:${String(id).padStart(2, "0")}:00Z`,
    updated_at: `2026-08-09T16:${String(id).padStart(2, "0")}:00Z`,
    user: { login: "agent" },
    author_association: "COLLABORATOR",
    ...overrides,
  };
}

function fenced(value) {
  return `\`\`\`phoenix-roadmap-update\n${JSON.stringify(value, null, 2)}\n\`\`\``;
}

function event(action, trigger) {
  return {
    action,
    issue: { number: 7 },
    comment: trigger,
    repository: { full_name: "owner/repo" },
  };
}

test("latest valid current comment wins per workstream", () => {
  const result = updatesFromComments([
    comment(1, fenced(update({ state: "Planning" }))),
    comment(2, fenced(update({ state: "Blocked" })), { user: { login: "ios-agent" } }),
  ]);

  assert.equal(result.length, 1);
  assert.equal(result[0].state, "Blocked");
  assert.equal(result[0].source.id, 2);
  assert.equal(result[0].source.author, "ios-agent");
});

test("edited comments remain current instead of silently deleting a workstream", () => {
  const edited = comment(1, fenced(update({ state: "Ready" })), {
    updated_at: "2026-08-09T17:00:00Z",
  });
  assert.equal(updatesFromComments([edited])[0].state, "Ready");
});

test("untrusted comments and invalid replacements are ignored", () => {
  const untrusted = comment(2, fenced(update({ state: "Ready" })), { author_association: "NONE" });
  const invalid = comment(3, fenced(update({ evidence: [] })));
  const result = updatesFromComments([comment(1, fenced(update())), untrusted, invalid]);

  assert.equal(result.length, 1);
  assert.equal(result[0].source.id, 1);
});

test("roadmap has fixed section and explicit order with verbatim context", () => {
  const comments = [
    comment(1, fenced(update({ workstream: "later", order: 20, title: "Later" }))),
    comment(2, fenced(update({ workstream: "first", order: 10, title: "First", context: "Line one\nLine two" }))),
    comment(3, fenced(update({ workstream: "p0", section: "parallel-p0", title: "P0", order: 90 }))),
  ];
  const body = renderRoadmap(updatesFromComments(comments), event("created", comments[2]));

  assert.ok(body.indexOf("### Parallel P0") < body.indexOf("### Blocked programs"));
  assert.ok(body.indexOf("<strong>First</strong>") < body.indexOf("<strong>Later</strong>"));
  assert.match(body, /> Line one\n> Line two/);
  assert.match(body, /_Reduced after agent comment created at 2026-08-09T16:03:00Z_/);
  assert.match(body, new RegExp(PROJECTION_START));
  assert.match(body, new RegExp(PROJECTION_END));
  assert.match(body, /This entire body is generated.*do not edit it manually/);
});

test("schema bounds each update before it can poison future reductions", () => {
  assert.throws(() => validateUpdate(update({ context: "x".repeat(801) })), /at most 800/);
  assert.throws(() => validateUpdate(update({ title: "x".repeat(201) })), /at most 200/);
  assert.throws(() => validateUpdate(update({ evidence: [] })), /between one and five/);
  assert.throws(() => validateUpdate(update({ section: "whatever" })), /unknown section/);
});

test("workstream count is bounded", () => {
  const comments = Array.from({ length: 13 }, (_, index) =>
    comment(index + 1, fenced(update({ workstream: `stream-${index}` }))),
  );
  assert.throws(() => updatesFromComments(comments), /at most 12/);
});

test("created and edited events rebuild the reducer-owned body", async () => {
  for (const action of ["created", "edited"]) {
    const trigger = comment(2, fenced(update()), {
      updated_at: action === "edited" ? "2026-08-09T17:00:00Z" : "2026-08-09T16:02:00Z",
    });
    const requests = [];
    const responses = [
      new Response(JSON.stringify([trigger]), { status: 200 }),
      new Response(JSON.stringify({}), { status: 200 }),
    ];
    const originalFetch = globalThis.fetch;
    globalThis.fetch = async (_url, options = {}) => {
      requests.push(options);
      return responses.shift();
    };
    try {
      const result = await run({ event: event(action, trigger), configuredIssueNumber: 7, token: "token" });
      assert.deepEqual(result, { changed: true, updates: 1 });
      const body = JSON.parse(requests.find((request) => request.method === "PATCH").body).body;
      assert.match(body, new RegExp(`comment ${action}`));
    } finally {
      globalThis.fetch = originalFetch;
    }
  }
});

test("editing an update into invalid content removes it immediately", async () => {
  const trigger = comment(2, "No structured update remains.", { updated_at: "2026-08-09T17:00:00Z" });
  const responses = [
    new Response(JSON.stringify([]), { status: 200 }),
    new Response(JSON.stringify({}), { status: 200 }),
  ];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => responses.shift();
  try {
    const result = await run({ event: event("edited", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 0 });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("deleted update rebuilds from remaining live comments", async () => {
  const deleted = comment(2, fenced(update({ workstream: "deleted" })));
  const remaining = comment(1, fenced(update({ workstream: "remaining", title: "Remaining" })));
  const responses = [
    new Response(JSON.stringify([remaining]), { status: 200 }),
    new Response(JSON.stringify({}), { status: 200 }),
  ];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => responses.shift();
  try {
    const result = await run({ event: event("deleted", deleted), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 1 });
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("untrusted trigger is rejected before API access", async () => {
  const trigger = comment(1, fenced(update()), { author_association: "NONE" });
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => assert.fail("untrusted trigger must not call GitHub");
  try {
    assert.deepEqual(
      await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" }),
      { skipped: "triggering author is not trusted" },
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});
