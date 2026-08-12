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

function event(action, trigger, overrides = {}) {
  return {
    action,
    issue: { number: 7 },
    comment: trigger,
    repository: { full_name: "owner/repo" },
    ...overrides,
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
    if (String(url).endsWith("/issues/7/comments") && method === "POST") {
      return new Response(JSON.stringify({}), { status: 201 });
    }
    if (String(url).includes("/reactions/") && method === "DELETE") {
      return new Response(null, { status: 204 });
    }
    if (String(url).includes("/comments?") && method === "GET") {
      return new Response(JSON.stringify(comments), { status: 200 });
    }
    if (String(url).endsWith("/issues/7") && method === "GET") {
      return new Response(JSON.stringify({ body: "" }), { status: 200 });
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

test("later retirement retains the original retirement ownership", () => {
  const current = comment(1, fenced(update()), { user: { login: "owner" } });
  const retirement = comment(2, retired("ios-vnext", 1), { user: { login: "owner" } });
  const takeover = comment(3, retired("ios-vnext", 1), { user: { login: "collaborator" } });
  assert.deepEqual(updatesFromComments([current, retirement, takeover]), []);
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

test("reactivation outside the projection supersedes its retirement", async () => {
  const base = Array.from({ length: 12 }, (_, index) =>
    comment(index + 1, fenced(update({ workstream: `stream-${index}`, order: index }))),
  );
  const original = comment(13, fenced(update({ workstream: "overflow", order: 99 })), { user: { login: "owner" } });
  const retirement = comment(14, retired("overflow", 13), { user: { login: "owner" } });
  const trigger = comment(15, fenced(update({ workstream: "overflow", order: 99 })), { user: { login: "owner" } });
  const mock = installGitHubMock([...base, original, retirement, trigger], [[], [], []]);
  try {
    const result = await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 12, acknowledged: "rejected" });
    const reactions = postedReactions(mock.requests);
    assert.deepEqual(reactions.slice(0, 2), ["eyes", "confused"]);
    assert.equal(reactions.filter((reaction) => reaction === "rocket").length, 12);
  } finally {
    mock.restore();
  }
});

test("projection change acknowledges the promoted source", async () => {
  const projected = Array.from({ length: 12 }, (_, index) =>
    comment(index + 1, fenced(update({ workstream: `stream-${index}`, order: index }))),
  );
  const promoted = comment(13, fenced(update({ workstream: "promoted", order: 12 })));
  const deleted = projected[0];
  const remaining = [...projected.slice(1), promoted];
  const mock = installGitHubMock(remaining, [[]]);
  try {
    await run({ event: event("deleted", deleted), configuredIssueNumber: 7, token: "token" });
    assert.ok(mock.requests.some(
      (request) => request.method === "POST" && request.url.includes("issues/comments/13/reactions"),
    ));
  } finally {
    mock.restore();
  }
});

test("coalesced run repairs missing rockets on projected sources", async () => {
  const existing = comment(1, fenced(update({ workstream: "existing", order: 0 })));
  const trigger = comment(2, fenced(update({ workstream: "trigger", order: 1 })));
  const mock = installGitHubMock([existing, trigger], [[], [], []]);
  try {
    await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.ok(mock.requests.some(
      (request) => request.method === "POST" && request.url.includes("issues/comments/1/reactions"),
    ));
  } finally {
    mock.restore();
  }
});

test("coalesced run repairs missing rockets on authoritative retirements", async () => {
  const current = comment(1, fenced(update()), { user: { login: "owner" } });
  const retirement = comment(2, retired("ios-vnext", 1), { user: { login: "owner" } });
  const trigger = comment(3, "Later coordination note.");
  const mock = installGitHubMock([current, retirement, trigger], [[], []]);
  try {
    await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.ok(mock.requests.some(
      (request) => request.method === "POST" && request.url.includes("issues/comments/2/reactions"),
    ));
  } finally {
    mock.restore();
  }
});

test("coalesced run marks invalid structured comments confused", async () => {
  const invalid = comment(1, fenced(update({ evidence: [] })));
  const trigger = comment(2, "Later coordination note.");
  const mock = installGitHubMock([invalid, trigger], [[], []]);
  try {
    await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.ok(mock.requests.some(
      (request) => request.method === "POST" &&
        request.url.includes("issues/comments/1/reactions") &&
        JSON.parse(request.body).content === "confused",
    ));
  } finally {
    mock.restore();
  }
});

test("editing an older retirement does not acknowledge it over the latest retirement", async () => {
  const updateComment = comment(1, fenced(update()), { user: { login: "owner" } });
  const trigger = comment(2, retired("ios-vnext", 1), { user: { login: "owner" } });
  const latest = comment(3, retired("ios-vnext", 1), { user: { login: "owner" } });
  const mock = installGitHubMock([updateComment, trigger, latest], [[], [], []]);
  try {
    const result = await run({ event: event("edited", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 0, acknowledged: "rejected" });
    assert.deepEqual(postedReactions(mock.requests), ["eyes", "confused", "rocket"]);
  } finally {
    mock.restore();
  }
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
  const mock = installGitHubMock([displaced, trigger], [[], [], [oldRocket]]);
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
    assert.deepEqual(postedReactions(mock.requests), ["eyes", "rocket", "confused"]);
  } finally {
    mock.restore();
  }
});

test("accepted retirement clears the retired source rocket", async () => {
  const current = comment(1, fenced(update()), { user: { login: "owner" } });
  const trigger = comment(2, retired("ios-vnext", 1), { user: { login: "owner" } });
  const oldRocket = { id: 61, content: "rocket", user: { login: "github-actions[bot]" } };
  const mock = installGitHubMock([current, trigger], [[], [], [oldRocket]]);
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
    assert.deepEqual(postedReactions(mock.requests), ["eyes", "confused", "rocket"]);
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
  const mock = installGitHubMock([remaining], [[]]);
  try {
    const result = await run({ event: event("deleted", deleted), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 1 });
    const body = JSON.parse(mock.requests.find((request) => request.method === "PATCH").body).body;
    assert.match(body, /Generated from the current trusted comment set/);
    assert.doesNotMatch(body, new RegExp(deleted.updated_at));
  } finally {
    mock.restore();
  }
});

test("ordinary trusted comments still rebuild from the live snapshot", async () => {
  const trigger = comment(2, "Ordinary coordination note.");
  const remaining = comment(1, fenced(update()));
  const responses = [
    new Response(JSON.stringify([remaining, trigger]), { status: 200 }),
    new Response(JSON.stringify([remaining, trigger]), { status: 200 }),
    new Response(JSON.stringify({}), { status: 200 }),
    new Response(JSON.stringify([remaining, trigger]), { status: 200 }),
    new Response(JSON.stringify({}), { status: 201 }),
    new Response(JSON.stringify([]), { status: 200 }),
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

test("deleting a replacement restores the reactivated source rocket", async () => {
  const reactivated = comment(1, fenced(update({ state: "Old" })));
  const deleted = comment(2, fenced(update({ state: "Deleted" })));
  const mock = installGitHubMock([reactivated], [[]]);
  try {
    const result = await run({ event: event("deleted", deleted), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 1 });
    assert.deepEqual(postedReactions(mock.requests), ["rocket"]);
    assert.ok(mock.requests.some(
      (request) => request.method === "POST" && request.url.includes("issues/comments/1/reactions"),
    ));
  } finally {
    mock.restore();
  }
});

test("deletion never fetches reactions from the deleted comment", async () => {
  const reactivated = comment(1, fenced(update({ state: "Old" })));
  const deleted = comment(2, fenced(update({ state: "Deleted" })));
  const mock = installGitHubMock([reactivated], [[]]);
  try {
    await run({ event: event("deleted", deleted), configuredIssueNumber: 7, token: "token" });
    assert.ok(!mock.requests.some(
      (request) => request.method === "GET" && request.url.includes("issues/comments/2/reactions"),
    ));
  } finally {
    mock.restore();
  }
});

test("editing a replacement away restores the reactivated source rocket", async () => {
  const reactivated = comment(1, fenced(update({ state: "Old" })));
  const trigger = comment(2, "Replacement removed.", { updated_at: "2026-08-09T17:00:00Z" });
  const mock = installGitHubMock([reactivated, trigger], [[], []]);
  try {
    const result = await run({
      event: event("edited", trigger, { changes: { body: { from: fenced(update({ state: "New" })) } } }),
      configuredIssueNumber: 7,
      token: "token",
    });
    assert.deepEqual(result, { changed: true, updates: 1 });
    assert.deepEqual(postedReactions(mock.requests), ["rocket"]);
  } finally {
    mock.restore();
  }
});

test("changing trusted snapshot retries before acknowledging", async () => {
  const trigger = comment(2, fenced(update()));
  const changed = comment(3, fenced(update({ workstream: "new" })));
  const requests = [];
  const responses = [
    new Response(JSON.stringify({}), { status: 201 }),
    new Response(JSON.stringify([]), { status: 200 }),
    new Response(JSON.stringify([trigger]), { status: 200 }),
    new Response(JSON.stringify([trigger, changed]), { status: 200 }),
    new Response(JSON.stringify({}), { status: 201 }),
    new Response(JSON.stringify([]), { status: 200 }),
  ];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, options = {}) => {
    requests.push({ url: String(url), method: options.method ?? "GET" });
    return responses.shift();
  };
  try {
    await assert.rejects(
      run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" }),
    );
    assert.ok(!requests.some((request) => request.method === "PATCH"));
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("untrusted retirement comments do not fetch reactions", async () => {
  const trigger = comment(2, "Trusted coordination note.");
  const outsider = comment(1, retired("ios-vnext", 99), { author_association: "NONE" });
  const mock = installGitHubMock([outsider, trigger]);
  try {
    await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" });
    assert.ok(!mock.requests.some(
      (request) => request.method === "GET" && request.url.includes("issues/comments/1/reactions"),
    ));
  } finally {
    mock.restore();
  }
});

test("edited retirement cannot reuse a stale acknowledgment", async () => {
  const trigger = comment(2, retired("missing", 99), {
    updated_at: "2026-08-09T18:00:00Z",
    user: { login: "owner" },
  });
  const staleRocket = {
    id: 71,
    content: "rocket",
    created_at: "2026-08-09T17:00:00Z",
    user: { login: "github-actions[bot]" },
  };
  const mock = installGitHubMock([trigger], [[], [staleRocket], []]);
  try {
    const result = await run({ event: event("edited", trigger), configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 0, acknowledged: "rejected" });
  } finally {
    mock.restore();
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
