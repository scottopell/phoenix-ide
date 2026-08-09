import assert from "node:assert/strict";
import test from "node:test";

import {
  PROJECTION_END,
  PROJECTION_START,
  renderProjection,
  replaceProjection,
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

function comment(id, body, author = "agent") {
  return {
    id,
    body,
    html_url: `https://github.test/issues/1#issuecomment-${id}`,
    created_at: `2026-08-09T16:${String(id).padStart(2, "0")}:00Z`,
    updated_at: `2026-08-09T16:${String(id).padStart(2, "0")}:00Z`,
    user: { login: author },
    author_association: "COLLABORATOR",
  };
}

function fenced(value) {
  return `\`\`\`phoenix-roadmap-update\n${JSON.stringify(value, null, 2)}\n\`\`\``;
}

test("latest valid comment wins per workstream and future comments are excluded", () => {
  const comments = [
    comment(1, fenced(update({ state: "Planning" }))),
    comment(2, fenced(update({ state: "Blocked" })), "ios-agent"),
    comment(3, fenced(update({ state: "Ready" }))),
  ];

  const result = updatesFromComments(comments, 2);

  assert.equal(result.length, 1);
  assert.equal(result[0].state, "Blocked");
  assert.equal(result[0].source.id, 2);
  assert.equal(result[0].source.author, "ios-agent");
});

test("updates from untrusted Issue participants are ignored", () => {
  const untrusted = { ...comment(1, fenced(update())), author_association: "NONE" };
  assert.deepEqual(updatesFromComments([untrusted], 1), []);
});

test("edited comments are excluded from the append-only record", () => {
  const edited = { ...comment(1, fenced(update())), updated_at: "2026-08-09T17:00:00Z" };
  assert.deepEqual(updatesFromComments([edited], 1), []);
});

test("invalid newer updates do not erase the latest valid update", () => {
  const invalid = update({ evidence: [] });
  const result = updatesFromComments(
    [comment(1, fenced(update())), comment(2, fenced(invalid))],
    2,
  );

  assert.equal(result.length, 1);
  assert.equal(result[0].source.id, 1);
});

test("projection has fixed section and explicit order with verbatim context", () => {
  const comments = [
    comment(1, fenced(update({ workstream: "later", order: 20, title: "Later" }))),
    comment(2, fenced(update({ workstream: "first", order: 10, title: "First", context: "Line one\nLine two" }))),
    comment(3, fenced(update({ workstream: "p0", section: "parallel-p0", title: "P0", order: 90 }))),
  ];
  const projection = renderProjection(updatesFromComments(comments, 3), comments[2]);

  assert.ok(projection.indexOf("### Parallel P0") < projection.indexOf("### Blocked programs"));
  assert.ok(projection.indexOf("<strong>First</strong>") < projection.indexOf("<strong>Later</strong>"));
  assert.match(projection, /> Line one\n> Line two/);
  assert.match(projection, /<!-- phoenix-roadmap:reduced-through:3 -->/);
  assert.match(projection, /_Reduced through agent comment at 2026-08-09T16:03:00Z_/);
});

test("projection replacement preserves coordinator-owned body exactly", () => {
  const original = `Coordinator preface\n\n${PROJECTION_START}\nold\n${PROJECTION_END}\n\n## Coordinator updates\n\nKeep this text.`;
  const projection = `${PROJECTION_START}\nnew\n${PROJECTION_END}`;

  assert.equal(
    replaceProjection(original, projection),
    `Coordinator preface\n\n${projection}\n\n## Coordinator updates\n\nKeep this text.`,
  );
});

test("projection is prepended when markers do not exist", () => {
  const projection = `${PROJECTION_START}\nnew\n${PROJECTION_END}`;
  assert.equal(replaceProjection("Coordinator notes", projection), `${projection}\n\nCoordinator notes`);
});

test("unmatched markers fail instead of overwriting coordinator content", () => {
  assert.throws(
    () => replaceProjection(`${PROJECTION_START}\nbroken`, "replacement"),
    /unmatched projection markers/,
  );
});

test("conditional retry preserves a coordinator edit made during reduction", async () => {
  const trigger = comment(2, fenced(update()));
  const event = {
    action: "created",
    issue: { number: 7 },
    comment: trigger,
    repository: { full_name: "owner/repo" },
  };
  const requests = [];
  const responses = [
    new Response(JSON.stringify([trigger]), { status: 200 }),
    new Response(JSON.stringify({ body: "Coordinator v1" }), { status: 200, headers: { etag: '"v1"' } }),
    new Response("", { status: 412 }),
    new Response(JSON.stringify({ body: "Coordinator v2" }), { status: 200, headers: { etag: '"v2"' } }),
    new Response(JSON.stringify({}), { status: 200 }),
  ];
  const originalFetch = globalThis.fetch;
  globalThis.fetch = async (_url, options = {}) => {
    requests.push(options);
    return responses.shift();
  };
  try {
    const result = await run({ event, configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { changed: true, updates: 1 });
    const patches = requests.filter((request) => request.method === "PATCH");
    assert.equal(patches[0].headers["If-Match"], '"v1"');
    assert.equal(patches[1].headers["If-Match"], '"v2"');
    assert.match(JSON.parse(patches[1].body).body, /Coordinator v2/);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("stale Action run cannot replace a newer projection", async () => {
  const trigger = comment(2, fenced(update()));
  const event = {
    action: "created",
    issue: { number: 7 },
    comment: trigger,
    repository: { full_name: "owner/repo" },
  };
  const originalFetch = globalThis.fetch;
  const requests = [];
  globalThis.fetch = async (_url, options = {}) => {
    requests.push(options);
    if (requests.length === 1) return new Response(JSON.stringify([trigger]), { status: 200 });
    return new Response(
      JSON.stringify({ body: `${PROJECTION_START}\n<!-- phoenix-roadmap:reduced-through:3 -->\n${PROJECTION_END}` }),
      { status: 200, headers: { etag: '"v3"' } },
    );
  };
  try {
    const result = await run({ event, configuredIssueNumber: 7, token: "token" });
    assert.deepEqual(result, { skipped: "a newer agent comment is already projected", updates: 1 });
    assert.equal(requests.some((request) => request.method === "PATCH"), false);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("schema rejects hidden inference inputs", () => {
  assert.throws(() => validateUpdate(update({ section: "whatever" })), /unknown section/);
  assert.throws(() => validateUpdate(update({ workstream: "iOS vNext" })), /kebab-case/);
  assert.throws(() => validateUpdate(update({ blocked_by: undefined })), /blocked_by/);
});
