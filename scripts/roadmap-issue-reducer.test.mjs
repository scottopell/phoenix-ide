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

test("edited events rebuild the reducer-owned body without lifecycle reactions", async () => {
  for (const action of ["edited"]) {
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
      assert.match(body, /Generated from the current trusted comment set/);
    } finally {
      globalThis.fetch = originalFetch;
    }
  }
});

test("created structured record transitions from eyes to rocket", async () => {
  const trigger = comment(2, fenced(update()));
  const requests = [];
  const responses = [
    new Response(JSON.stringify({}), { status: 201 }),
    new Response(JSON.stringify([]), { status: 200 }),
    new Response(JSON.stringify([trigger]), { status: 200 }),
    new Response(JSON.stringify({}), { status: 200 }),
    new Response(JSON.stringify({}), { status: 201 }),
    new Response(JSON.stringify([{ id: 10, content: "eyes", user: { login: "github-actions[bot]" } }]), { status: 200 }),
    new Response(null, { status: 204 }),
  ];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, options = {}) => {
    requests.push({ url: String(url), ...options });
    return responses.shift();
  };
  try {
    assert.deepEqual(
      await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" }),
      { changed: true, updates: 1, acknowledged: "accepted" },
    );
    const reactions = requests
      .filter((request) => request.method === "POST" && request.url.includes("/reactions"))
      .map((request) => JSON.parse(request.body).content);
    assert.deepEqual(reactions, ["eyes", "rocket"]);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("created retirement is accepted when its workstream is absent", async () => {
  const trigger = comment(2, retired("ios-vnext", 1));
  const responses = [
    new Response(JSON.stringify({}), { status: 201 }),
    new Response(JSON.stringify([]), { status: 200 }),
    new Response(JSON.stringify([trigger]), { status: 200 }),
    new Response(JSON.stringify({}), { status: 200 }),
    new Response(JSON.stringify({}), { status: 201 }),
    new Response(JSON.stringify([]), { status: 200 }),
  ];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async () => responses.shift();
  try {
    assert.deepEqual(
      await run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" }),
      { changed: true, updates: 0, acknowledged: "accepted" },
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("processing transition failure attempts a confused terminal reaction", async () => {
  const trigger = comment(2, fenced(update()));
  const posts = [];
  let call = 0;
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (url, options = {}) => {
    call += 1;
    if (options.method === "POST") posts.push(JSON.parse(options.body).content);
    if (call === 1) return new Response(JSON.stringify({}), { status: 201 });
    if (call === 2) return new Response("temporary", { status: 500 });
    if (call === 3) return new Response(JSON.stringify({}), { status: 201 });
    return new Response(JSON.stringify([]), { status: 200 });
  };
  try {
    await assert.rejects(run({ event: event("created", trigger), configuredIssueNumber: 7, token: "token" }));
    assert.deepEqual(posts, ["eyes", "confused"]);
  } finally {
    globalThis.fetch = originalFetch;
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
