import assert from "node:assert/strict";
import test from "node:test";

import {
  PROJECTION_END,
  PROJECTION_START,
  renderRoadmap,
  run,
  updatesFromComments,
  validateRetirement,
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

function retired(workstream = "ios-vnext", supersedesCommentId = 1) {
  return `\`\`\`phoenix-roadmap-retirement\n${JSON.stringify({ kind: "workstream-retirement", version: 1, workstream, supersedes_comment_id: supersedesCommentId }, null, 2)}\n\`\`\``;
}

function event(action, trigger) {
  return {
    action,
    issue: { number: 7 },
    comment: trigger,
    repository: { full_name: "owner/repo" },
  };
}

function installGitHubMock(comments, reactionSnapshots = []) {
  const requests = [];
  const snapshots = [...reactionSnapshots];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, options = {}) => {
    const method = options.method ?? "GET";
    requests.push({ url: String(url), ...options, method });
    if (String(url).includes("/reactions") && method === "GET") {
      return new Response(JSON.stringify(snapshots.shift() ?? []), { status: 200 });
    }
    if (String(url).includes("/reactions") && method === "POST") {
      return new Response(JSON.stringify({}), { status: 201 });
    }
    if (String(url).includes("/reactions/") && method === "DELETE") {
      return new Response(null, { status: 204 });
    }
    if (String(url).includes("/comments?") && method === "GET") {
      return new Response(JSON.stringify(comments), { status: 200 });
    }
    if (method === "PATCH") return new Response(JSON.stringify({}), { status: 200 });
    throw new Error(`Unexpected mock request: ${method} ${url}`);
  };
  return { requests, restore: () => { globalThis.fetch = originalFetch; } };
}

function postedReactions(requests) {
  return requests
    .filter((request) => request.method === "POST" && request.url.includes("/reactions"))
    .map((request) => JSON.parse(request.body).content);
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

test("retirement removes the prior workstream and a later update can reactivate it", () => {
  const comments = [
    comment(1, fenced(update())),
    comment(2, retired()),
  ];
  assert.deepEqual(updatesFromComments(comments), []);
  assert.equal(updatesFromComments([...comments, comment(3, fenced(update({ state: "Restarted" })))])[0].state, "Restarted");
});

test("retirement has a minimal exact schema bound to a source comment", () => {
  assert.deepEqual(
    validateRetirement({ kind: "workstream-retirement", version: 1, workstream: "ios-vnext", supersedes_comment_id: 42 }),
    { kind: "workstream-retirement", version: 1, workstream: "ios-vnext", supersedes_comment_id: 42 },
  );
  assert.throws(
    () => validateRetirement({ kind: "workstream-retirement", version: 1, workstream: "ios-vnext", supersedes_comment_id: 42, state: "Done" }),
    /only kind, version, workstream, and supersedes_comment_id/,
  );
});

test("retirement requires the same author and cannot retire a newer source", () => {
  const current = comment(1, fenced(update()), { user: { login: "owner" } });
  const wrongAuthor = comment(2, retired("ios-vnext", 1), { user: { login: "other" } });
  const newer = comment(3, fenced(update({ state: "Newer" })), { user: { login: "owner" } });
  const staleRetirement = comment(4, retired("ios-vnext", 1), { user: { login: "owner" } });
  const matching = comment(5, retired("ios-vnext", 3), { user: { login: "owner" } });
  assert.equal(updatesFromComments([current, wrongAuthor]).length, 1);
  assert.equal(updatesFromComments([current, newer, staleRetirement])[0].state, "Newer");
  assert.equal(updatesFromComments([current, newer, matching]).length, 0);
});

test("retirement remains authoritative if its superseded source disappears", () => {
  const older = comment(1, fenced(update({ state: "Older" })), { user: { login: "owner" } });
  const retiredCurrent = comment(3, retired("ios-vnext", 2), { user: { login: "owner" } });

  assert.equal(updatesFromComments([older, retiredCurrent]).length, 0);
  assert.equal(updatesFromComments([retiredCurrent]).length, 0);
});

test("edited comments remain current instead of silently deleting a workstream", () => {
  const edited = comment(1, fenced(update({ state: "Ready" })), {
    updated_at: "2026-08-09T17:00:00Z",
  });
  assert.equal(updatesFromComments([edited])[0].state, "Ready");
});

test("roadmap-update examples nested in larger fences are not live updates", () => {
  const nested = [
    "````markdown",
    "```phoenix-roadmap-update",
    JSON.stringify(update()),
    "```",
    "````",
  ].join("\n");
  assert.deepEqual(updatesFromComments([comment(1, nested)]), []);
});

test("roadmap-update fences with surrounding or hidden content are not live", () => {
  const live = fenced(update());
  assert.deepEqual(updatesFromComments([comment(1, `Prose\n${live}`)]), []);
  assert.deepEqual(updatesFromComments([comment(2, `<!--\n${live}\n-->`)]), []);
  assert.deepEqual(updatesFromComments([comment(3, `<pre>\n${live}\n</pre>`)]), []);
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
  const body = renderRoadmap(updatesFromComments(comments));

  assert.ok(body.indexOf("### Parallel P0") < body.indexOf("### Blocked programs"));
  assert.ok(body.indexOf("<strong>First</strong>") < body.indexOf("<strong>Later</strong>"));
  assert.match(body, /> Line one\n> Line two/);
  assert.match(body, /_Generated from the current trusted comment set\._/);
  assert.match(body, new RegExp(PROJECTION_START));
  assert.match(body, new RegExp(PROJECTION_END));
  assert.match(body, /This entire body is generated.*do not edit it manually/);
  assert.match(body, /<!-- phoenix-roadmap:snapshot-through:0 -->/);
});

test("schema bounds each update before it can poison future reductions", () => {
  assert.throws(() => validateUpdate(update({ context: "x".repeat(801) })), /at most 800/);
  assert.throws(() => validateUpdate(update({ title: "x".repeat(201) })), /at most 200/);
  assert.throws(() => validateUpdate(update({ evidence: [] })), /between one and five/);
  assert.throws(() => validateUpdate(update({ section: "whatever" })), /unknown section/);
});

test("workstream overflow is deterministically omitted without wedging reduction", () => {
  const comments = Array.from({ length: 13 }, (_, index) =>
    comment(index + 1, fenced(update({ workstream: `stream-${index}`, order: index }))),
  );
  const result = updatesFromComments(comments);
  assert.equal(result.length, 12);
  assert.deepEqual(result.map(({ workstream }) => workstream), Array.from({ length: 12 }, (_, index) => `stream-${index}`));
});

test("evidence URL uses its dedicated 500-character limit", () => {
  const longUrl = `https://example.test/${"x".repeat(470)}`;
  assert.equal(validateUpdate(update({ evidence: [{ label: "long", url: longUrl }] })).evidence[0].url.length > 200, true);
});

test("context HTML is escaped inside details markup", () => {
  const source = comment(1, fenced(update({ context: "</details><strong>escape me</strong>" })));
  const body = renderRoadmap(updatesFromComments([source]));
  assert.doesNotMatch(body, /> <\/details>/);
  assert.match(body, /\\<\/details\\>/);
});

test("summary fields use HTML entity escaping", () => {
  const source = comment(1, fenced(update({ title: "<!-- title", state: "<ready> & done" })));
  const body = renderRoadmap(updatesFromComments([source]));
  assert.match(body, /&lt;!-- title/);
  assert.match(body, /&lt;ready&gt; &amp; done/);
  assert.doesNotMatch(body, /<summary><strong><!--/);
});

test("created and edited records transition from eyes to rocket after projection", async () => {
  for (const action of ["created", "edited"]) {
    const trigger = comment(2, fenced(update()), {
      updated_at: action === "edited" ? "2026-08-09T17:00:00Z" : "2026-08-09T16:02:00Z",
    });
    const mock = installGitHubMock([trigger]);
    try {
      const result = await run({ event: event(action, trigger), configuredIssueNumber: 7, token: "token" });
      assert.deepEqual(result, { changed: true, updates: 1, acknowledged: "accepted" });
      const body = JSON.parse(mock.requests.find((request) => request.method === "PATCH").body).body;
      assert.match(body, /Generated from the current trusted comment set/);
      assert.match(body, /issuecomment-2/);
      assert.deepEqual(postedReactions(mock.requests), ["eyes", "rocket"]);
    } finally {
      mock.restore();
    }
  }
});

test("terminal reaction is posted before the processing reaction is removed", async () => {
  const trigger = comment(2, fenced(update()));
  const eyes = { id: 41, content: "eyes", user: { login: "github-actions[bot]" } };
  const mock = installGitHubMock([trigger], [[], [eyes]]);
  try {
    await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    const terminalPost = mock.requests.findIndex(
      (request) => request.method === "POST" && JSON.parse(request.body).content === "rocket",
    );
    const eyesDelete = mock.requests.findIndex(
      (request) => request.method === "DELETE" && request.url.endsWith("/reactions/41"),
    );
    assert.ok(terminalPost >= 0);
    assert.ok(eyesDelete > terminalPost);
  } finally {
    mock.restore();
  }
});

test("reaction cleanup follows pagination", async () => {
  const trigger = comment(2, fenced(update()));
  const firstPage = Array.from({ length: 100 }, (_, index) => ({
    id: index + 1,
    content: "heart",
    user: { login: `reviewer-${index}` },
  }));
  const eyes = { id: 101, content: "eyes", user: { login: "github-actions[bot]" } };
  const mock = installGitHubMock([trigger], [firstPage, [], firstPage, [eyes]]);
  try {
    await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.ok(mock.requests.some(
      (request) => request.method === "DELETE" && request.url.endsWith("/reactions/101"),
    ));
    assert.ok(mock.requests.some(
      (request) => request.method === "GET" && request.url.includes("per_page=100&page=2"),
    ));
  } finally {
    mock.restore();
  }
});

test("replacement update clears the displaced source rocket", async () => {
  const displaced = comment(1, fenced(update({ state: "Old" })));
  const trigger = comment(2, fenced(update({ state: "New" })));
  const oldRocket = { id: 51, content: "rocket", user: { login: "github-actions[bot]" } };
  const mock = installGitHubMock([displaced, trigger], [[], [oldRocket], []]);
  try {
    const result = await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 1, acknowledged: "accepted" });
    assert.ok(mock.requests.some(
      (request) => request.method === "DELETE" && request.url.endsWith("/reactions/51"),
    ));
  } finally {
    mock.restore();
  }
});

test("invalid structured record transitions from eyes to confused", async () => {
  const trigger = comment(2, fenced(update({ evidence: [] })));
  const mock = installGitHubMock([trigger]);
  try {
    const result = await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 0, acknowledged: "rejected" });
    assert.deepEqual(postedReactions(mock.requests), ["eyes", "confused"]);
  } finally {
    mock.restore();
  }
});

test("accepted retirement transitions from eyes to rocket after removing the entry", async () => {
  const current = comment(1, fenced(update()), { user: { login: "owner" } });
  const trigger = comment(2, retired("ios-vnext", 1), { user: { login: "owner" } });
  const mock = installGitHubMock([current, trigger]);
  try {
    const result = await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 0, acknowledged: "accepted" });
    const body = JSON.parse(mock.requests.find((request) => request.method === "PATCH").body).body;
    assert.doesNotMatch(body, /issuecomment-1/);
    assert.deepEqual(postedReactions(mock.requests), ["eyes", "rocket"]);
  } finally {
    mock.restore();
  }
});

test("accepted retirement clears the retired source rocket", async () => {
  const current = comment(1, fenced(update()), { user: { login: "owner" } });
  const trigger = comment(2, retired("ios-vnext", 1), { user: { login: "owner" } });
  const oldRocket = { id: 61, content: "rocket", user: { login: "github-actions[bot]" } };
  const mock = installGitHubMock([current, trigger], [[], [oldRocket], []]);
  try {
    await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.ok(mock.requests.some(
      (request) => request.method === "DELETE" && request.url.endsWith("/reactions/61"),
    ));
  } finally {
    mock.restore();
  }
});

test("stale retirement transitions from eyes to confused", async () => {
  const current = comment(1, fenced(update()), { user: { login: "owner" } });
  const newer = comment(2, fenced(update({ state: "Newer" })), { user: { login: "owner" } });
  const trigger = comment(3, retired("ios-vnext", 1), { user: { login: "owner" } });
  const mock = installGitHubMock([current, newer, trigger]);
  try {
    const result = await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 1, acknowledged: "rejected" });
    assert.deepEqual(postedReactions(mock.requests), ["eyes", "confused"]);
  } finally {
    mock.restore();
  }
});

test("editing an update away removes its projection and lifecycle reaction", async () => {
  const trigger = comment(2, "No structured update remains.", { updated_at: "2026-08-09T17:00:00Z" });
  const rocket = { id: 42, content: "rocket", user: { login: "github-actions[bot]" } };
  const otherUser = { id: 43, content: "rocket", user: { login: "reviewer" } };
  const mock = installGitHubMock([], [[rocket, otherUser]]);
  try {
    const result = await run({ event: event("edited", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 0 });
    assert.deepEqual(postedReactions(mock.requests), []);
    assert.ok(mock.requests.some(
      (request) => request.method === "DELETE" && request.url.endsWith("/reactions/42"),
    ));
    assert.ok(!mock.requests.some(
      (request) => request.method === "DELETE" && request.url.endsWith("/reactions/43"),
    ));
  } finally {
    mock.restore();
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
  const requests = [];
  globalThis.fetch = async (_url, options = {}) => {
    requests.push(options);
    return responses.shift();
  };
  try {
    const result = await run({ event: event("deleted", deleted), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 1 });
    const body = JSON.parse(requests.find((request) => request.method === "PATCH").body).body;
    assert.match(body, /Generated from the current trusted comment set/);
    assert.doesNotMatch(body, new RegExp(deleted.updated_at));
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("ordinary trusted comments still rebuild from the live snapshot", async () => {
  const trigger = comment(2, "Ordinary coordination note.");
  const remaining = comment(1, fenced(update()));
  const responses = [
    new Response(JSON.stringify([remaining, trigger]), { status: 200 }),
    new Response(JSON.stringify({}), { status: 200 }),
  ];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => responses.shift();
  try {
    assert.deepEqual(
      await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" }),
      { changed: true, updates: 1 },
    );
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
