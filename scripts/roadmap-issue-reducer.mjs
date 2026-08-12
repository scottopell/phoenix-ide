#!/usr/bin/env node

import fs from "node:fs/promises";
import process from "node:process";
import { pathToFileURL } from "node:url";

export const PROJECTION_START = "<!-- phoenix-roadmap:projection:start -->";
export const PROJECTION_END = "<!-- phoenix-roadmap:projection:end -->";
const MAX_UPDATE_BYTES = 2_000;
const MAX_WORKSTREAMS = 12;
const MAX_ISSUE_BODY_BYTES = 65_536;

const SECTION_ORDER = [
  ["critical-path", "Critical path"],
  ["parallel-p0", "Parallel P0"],
  ["active-work", "Active work"],
  ["blocked-programs", "Blocked programs"],
  ["queued-programs", "Queued programs"],
];
const VALID_SECTIONS = new Set(SECTION_ORDER.map(([key]) => key));
const TRUSTED_ASSOCIATIONS = new Set(["OWNER", "MEMBER", "COLLABORATOR"]);

function requiredLine(value, field) {
  if (typeof value !== "string" || value.trim() === "" || /[\r\n]/.test(value)) {
    throw new Error(`${field} must be a non-empty single-line string`);
  }
  const trimmed = value.trim();
  if (trimmed.length > 200) throw new Error(`${field} must be at most 200 characters`);
  return trimmed;
}

function requiredUrl(value, field) {
  if (typeof value !== "string" || value.trim() === "" || /[\r\n]/.test(value)) {
    throw new Error(`${field} must be a non-empty single-line string`);
  }
  const trimmed = value.trim();
  if (trimmed.length > 500) throw new Error(`${field} must be at most 500 characters`);
  return trimmed;
}

function optionalText(value, field) {
  if (value === undefined || value === null || value === "") return "";
  if (typeof value !== "string") throw new Error(`${field} must be a string`);
  if (value.includes(PROJECTION_START) || value.includes(PROJECTION_END)) {
    throw new Error(`${field} contains a reserved projection marker`);
  }
  const trimmed = value.trim();
  if (trimmed.length > 800) throw new Error(`${field} must be at most 800 characters`);
  return trimmed;
}

function validateEvidence(value) {
  if (!Array.isArray(value) || value.length === 0 || value.length > 5) {
    throw new Error("evidence must contain between one and five links");
  }
  return value.map((item, index) => {
    if (item === null || typeof item !== "object" || Array.isArray(item)) {
      throw new Error(`evidence[${index}] must be an object`);
    }
    const label = requiredLine(item.label, `evidence[${index}].label`);
    const url = requiredUrl(item.url, `evidence[${index}].url`);
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
      throw new Error(`evidence[${index}].url must use http or https`);
    }
    return { label, url: parsed.toString() };
  });
}

function validateWorkstream(value) {
  const workstream = requiredLine(value, "workstream");
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(workstream)) {
    throw new Error("workstream must be a lowercase kebab-case identifier");
  }
  return workstream;
}

export function validateUpdate(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("update must be an object");
  }
  if (Buffer.byteLength(JSON.stringify(value), "utf8") > MAX_UPDATE_BYTES) {
    throw new Error(`update must be at most ${MAX_UPDATE_BYTES} UTF-8 bytes`);
  }
  if (value.kind !== "workstream-update" || value.version !== 1) {
    throw new Error("update must have kind workstream-update and version 1");
  }

  const workstream = validateWorkstream(value.workstream);
  const section = requiredLine(value.section, "section");
  if (!VALID_SECTIONS.has(section)) throw new Error(`unknown section: ${section}`);
  const order = value.order ?? 100;
  if (!Number.isSafeInteger(order) || order < 0) {
    throw new Error("order must be a non-negative integer");
  }
  if (
    !Array.isArray(value.blocked_by) ||
    value.blocked_by.length > 5 ||
    value.blocked_by.some((item) => typeof item !== "string" || item.trim() === "" || item.length > 200 || /[\r\n]/.test(item))
  ) {
    throw new Error("blocked_by must contain at most five non-empty single-line strings of at most 200 characters");
  }

  return {
    kind: value.kind,
    version: value.version,
    workstream,
    title: requiredLine(value.title, "title"),
    state: requiredLine(value.state, "state"),
    owner: requiredLine(value.owner, "owner"),
    blocked_by: value.blocked_by.map((item) => item.trim()),
    next: requiredLine(value.next, "next"),
    section,
    order,
    evidence: validateEvidence(value.evidence),
    context: optionalText(value.context, "context"),
  };
}

export function validateRetirement(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("retirement must be an object");
  }
  const keys = Object.keys(value).sort();
  if (keys.join(",") !== "kind,supersedes_comment_id,version,workstream") {
    throw new Error("retirement must contain only kind, version, workstream, and supersedes_comment_id");
  }
  if (value.kind !== "workstream-retirement" || value.version !== 1) {
    throw new Error("retirement must have kind workstream-retirement and version 1");
  }
  if (!Number.isSafeInteger(value.supersedes_comment_id) || value.supersedes_comment_id <= 0) {
    throw new Error("supersedes_comment_id must be a positive integer");
  }
  return {
    kind: value.kind,
    version: value.version,
    workstream: validateWorkstream(value.workstream),
    supersedes_comment_id: value.supersedes_comment_id,
  };
}

function roadmapPayload(markdown) {
  const match = String(markdown ?? "").match(/^```phoenix-roadmap-(update|retirement)\r?\n([\s\S]*?)\r?\n```\s*$/);
  return match ? { recordType: match[1], payload: match[2] } : null;
}

export function reduceComments(comments) {
  const latest = new Map();
  const outcomes = new Map();
  const ordered = [...comments]
    .filter(
      (comment) =>
        Number.isSafeInteger(comment.id) &&
        TRUSTED_ASSOCIATIONS.has(comment.author_association),
    )
    .sort((left, right) => left.id - right.id);

  for (const comment of ordered) {
    const record = roadmapPayload(comment.body);
    if (!record) continue;
    try {
      const value = JSON.parse(record.payload);
      if (record.recordType === "retirement") {
        const retirement = validateRetirement(value);
        const current = latest.get(retirement.workstream);
        const retirementAuthor = comment.user?.login ?? "unknown";
        if (
          current === undefined ||
          (current.source.id <= retirement.supersedes_comment_id && current.source.author === retirementAuthor)
        ) {
          latest.delete(retirement.workstream);
          outcomes.set(comment.id, { accepted: true, recordType: "retirement", workstream: retirement.workstream });
        } else {
          const reason = "retirement must supersede the same author's current source";
          outcomes.set(comment.id, { accepted: false, reason, recordType: "retirement", workstream: retirement.workstream });
          console.warn(`Ignoring retirement in comment ${comment.id}: ${reason}`);
        }
        continue;
      }
      const update = validateUpdate(value);
      latest.set(update.workstream, {
        ...update,
        source: {
          id: comment.id,
          url: comment.html_url,
          author: comment.user?.login ?? "unknown",
          created_at: comment.created_at,
        },
      });
      outcomes.set(comment.id, { accepted: true, recordType: "update", workstream: update.workstream });
    } catch (error) {
      outcomes.set(comment.id, { accepted: false, reason: error.message, recordType: record.recordType });
      console.warn(`Ignoring invalid roadmap record in comment ${comment.id}: ${error.message}`);
    }
  }

  const current = [...latest.values()];
  const orderedUpdates = current.sort((left, right) => {
    const sectionOrder =
      SECTION_ORDER.findIndex(([key]) => key === left.section) -
      SECTION_ORDER.findIndex(([key]) => key === right.section);
    if (sectionOrder !== 0) return sectionOrder;
    if (left.order !== right.order) return left.order - right.order;
    return left.workstream < right.workstream ? -1 : left.workstream > right.workstream ? 1 : 0;
  });
  if (orderedUpdates.length > MAX_WORKSTREAMS) {
    console.warn(`Roadmap has ${orderedUpdates.length} workstreams; projecting the first ${MAX_WORKSTREAMS} by explicit roadmap order`);
  }
  return { updates: orderedUpdates.slice(0, MAX_WORKSTREAMS), current, outcomes };
}

export function updatesFromComments(comments) {
  return reduceComments(comments).updates;
}

function markdownText(value) {
  return value.replaceAll("\\", "\\\\").replace(/([`*_{}\[\]<>])/g, "\\$1");
}

function htmlText(value) {
  return value
    .replaceAll("&", "&amp;")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .replaceAll('"', "&quot;")
    .replaceAll("'", "&#39;");
}

function renderEvidence(evidence) {
  return evidence.map(({ label, url }) => `[${markdownText(label)}](${url})`).join(" · ");
}

function renderContext(context) {
  if (!context) return "";
  return `\n\nAgent context:\n\n${context.split("\n").map((line) => `> ${markdownText(line)}`).join("\n")}`;
}

function renderUpdate(update, open) {
  const blockers = update.blocked_by.length === 0 ? "None" : update.blocked_by.map(markdownText).join(" · ");
  const sourceLabel = `@${update.source.author} update`;
  return `<details${open ? " open" : ""}>\n<summary><strong>${htmlText(update.title)}</strong> — ${htmlText(update.state)}</summary>\n\nOwner: ${markdownText(update.owner)}  \nBlocked by: ${blockers}  \nNext: ${markdownText(update.next)}  \nEvidence: ${renderEvidence(update.evidence)}  \nSource: [${markdownText(sourceLabel)}](${update.source.url})${renderContext(update.context)}\n\n</details>`;
}

export function renderRoadmap(updates, snapshotThroughCommentId = 0) {
  const lines = [
    "# Phoenix delivery roadmap",
    "",
    "One-request orientation for current Phoenix delivery. This entire body is generated from trusted structured comments; do not edit it manually.",
    "",
    PROJECTION_START,
    `<!-- phoenix-roadmap:snapshot-through:${snapshotThroughCommentId} -->`,
    "",
    "## Current roadmap",
    "",
    "_Generated from the current trusted comment set._",
  ];

  for (const [section, heading] of SECTION_ORDER) {
    const entries = updates.filter((update) => update.section === section);
    if (entries.length === 0) continue;
    lines.push("", `### ${heading}`, "");
    lines.push(entries.map((update) => renderUpdate(update, section === "critical-path" || section === "parallel-p0")).join("\n\n"));
  }

  if (updates.length === 0) {
    lines.push("", "_No valid agent updates have been appended yet._");
  }
  lines.push(
    "",
    PROJECTION_END,
    "",
    "## Roadmap rules",
    "",
    "- ProductConversation is the primary P0 program.",
    "- Independent P0 repairs may proceed when they do not destabilize that critical path.",
    "- Requirements remain in specs/Allium; decisions in ADRs; review state in PRs; shipped reality on `main`.",
  );
  const body = lines.join("\n");
  if (Buffer.byteLength(body, "utf8") > MAX_ISSUE_BODY_BYTES) {
    throw new Error(`generated roadmap exceeds GitHub's ${MAX_ISSUE_BODY_BYTES}-byte Issue body limit`);
  }
  return body;
}

async function githubResponse(path, token, options = {}) {
  return fetch(`https://api.github.com${path}`, {
    ...options,
    headers: {
      Accept: "application/vnd.github+json",
      Authorization: `Bearer ${token}`,
      "X-GitHub-Api-Version": "2022-11-28",
      "User-Agent": "phoenix-roadmap-issue-reducer",
      ...options.headers,
    },
  });
}

async function githubRequest(path, token, options = {}) {
  const response = await githubResponse(path, token, options);
  if (!response.ok) throw new Error(`GitHub ${options.method ?? "GET"} ${path}: ${response.status} ${await response.text()}`);
  return response.status === 204 ? null : response.json();
}

async function replaceIssueBody(owner, repo, issueNumber, body, token) {
  await githubRequest(`/repos/${owner}/${repo}/issues/${issueNumber}`, token, {
    method: "PATCH",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ body }),
  });
  return { changed: true };
}

async function listComments(owner, repo, issueNumber, token) {
  const comments = [];
  for (let page = 1; ; page += 1) {
    const batch = await githubRequest(`/repos/${owner}/${repo}/issues/${issueNumber}/comments?per_page=100&page=${page}`, token);
    comments.push(...batch);
    if (batch.length < 100) return comments;
  }
}

const LIFECYCLE_REACTIONS = new Set(["eyes", "rocket", "confused"]);

async function listReactions(owner, repo, commentId, token) {
  const reactions = [];
  for (let page = 1; ; page += 1) {
    const batch = await githubRequest(
      `/repos/${owner}/${repo}/issues/comments/${commentId}/reactions?per_page=100&page=${page}`,
      token,
    );
    reactions.push(...batch);
    if (batch.length < 100) return reactions;
  }
}

async function clearLifecycleReactions(owner, repo, commentId, token, except = null) {
  const reactions = await listReactions(owner, repo, commentId, token);
  for (const reaction of reactions) {
    if (
      reaction.user?.login === "github-actions[bot]" &&
      LIFECYCLE_REACTIONS.has(reaction.content) &&
      reaction.content !== except
    ) {
      await githubRequest(`/repos/${owner}/${repo}/issues/comments/${commentId}/reactions/${reaction.id}`, token, {
        method: "DELETE",
      });
    }
  }
}

async function clearAcceptedReaction(owner, repo, commentId, token) {
  const reactions = await listReactions(owner, repo, commentId, token);
  for (const reaction of reactions) {
    if (reaction.user?.login === "github-actions[bot]" && reaction.content === "rocket") {
      await githubRequest(`/repos/${owner}/${repo}/issues/comments/${commentId}/reactions/${reaction.id}`, token, {
        method: "DELETE",
      });
    }
  }
}

function acknowledgedCommentIds(updates, current, outcomes) {
  const acknowledged = new Set(updates.map((update) => update.source.id));
  const activeWorkstreams = new Set(current.map((update) => update.workstream));
  const latestRetirements = new Map();
  for (const [commentId, outcome] of outcomes) {
    if (outcome.accepted && outcome.recordType === "retirement" && !activeWorkstreams.has(outcome.workstream)) {
      latestRetirements.set(outcome.workstream, commentId);
    }
  }
  for (const commentId of latestRetirements.values()) acknowledged.add(commentId);
  return acknowledged;
}

function commentsBeforeEvent(comments, event) {
  if (event.action === "created") return comments.filter((comment) => comment.id !== event.comment.id);
  if (event.action === "deleted") return [...comments, event.comment];
  if (event.action === "edited" && typeof event.changes?.body?.from === "string") {
    return comments.map((comment) => comment.id === event.comment.id
      ? { ...comment, body: event.changes.body.from }
      : comment);
  }
  return comments;
}

async function reconcileReactions(owner, repo, before, after, projectedUpdates, outcomes, triggerId, deletedId, token) {
  for (const commentId of before) {
    if (commentId !== triggerId && commentId !== deletedId && !after.has(commentId)) {
      await clearAcceptedReaction(owner, repo, commentId, token);
    }
  }

  const ensureAccepted = new Set(projectedUpdates.map((update) => update.source.id));
  for (const commentId of after) ensureAccepted.add(commentId);
  ensureAccepted.delete(triggerId);
  for (const commentId of ensureAccepted) {
    await setLifecycleReaction(owner, repo, commentId, "rocket", token);
  }

  for (const [commentId, outcome] of outcomes) {
    if (commentId !== triggerId && !outcome.accepted) {
      await setLifecycleReaction(owner, repo, commentId, "confused", token);
    }
  }
}

async function setLifecycleReaction(owner, repo, commentId, content, token) {
  const path = `/repos/${owner}/${repo}/issues/comments/${commentId}/reactions`;
  await githubRequest(path, token, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ content }),
  });
  await clearLifecycleReactions(owner, repo, commentId, token, content);
}

function isStructuredRoadmapComment(comment) {
  return roadmapPayload(comment?.body) !== null;
}

export async function run({ event, configuredIssueNumber, token }) {
  if (!["created", "edited", "deleted"].includes(event.action) || event.issue?.pull_request) {
    return { skipped: "not a supported Issue comment event" };
  }
  if (event.issue?.number !== configuredIssueNumber) return { skipped: "not the configured roadmap Issue" };
  if (!Number.isSafeInteger(event.comment?.id) || !event.comment?.created_at) throw new Error("event lacks a triggering comment identity");
  if (!TRUSTED_ASSOCIATIONS.has(event.comment.author_association)) {
    return { skipped: "triggering author is not trusted" };
  }
  const [owner, repo] = event.repository.full_name.split("/");
  const tracksLifecycle = event.action !== "deleted" && isStructuredRoadmapComment(event.comment);

  let projectionCommitted = false;
  try {
    if (tracksLifecycle) {
      await setLifecycleReaction(owner, repo, event.comment.id, "eyes", token);
    } else if (event.action === "edited") {
      await clearLifecycleReactions(owner, repo, event.comment.id, token);
    }
    const comments = await listComments(owner, repo, configuredIssueNumber, token);
    const trustedCommentIds = comments
      .filter((comment) => Number.isSafeInteger(comment.id) && TRUSTED_ASSOCIATIONS.has(comment.author_association))
      .map((comment) => comment.id);
    const snapshotThroughCommentId = trustedCommentIds.length === 0 ? 0 : Math.max(...trustedCommentIds);
    const { updates, current, outcomes } = reduceComments(comments);
    const before = reduceComments(commentsBeforeEvent(comments, event));
    const body = renderRoadmap(updates, snapshotThroughCommentId);
    const changed = await replaceIssueBody(owner, repo, configuredIssueNumber, body, token);
    projectionCommitted = true;

    let reflected = false;
    if (tracksLifecycle) {
      const outcome = outcomes.get(event.comment.id);
      const acknowledgedIds = acknowledgedCommentIds(updates, current, outcomes);
      reflected = outcome?.accepted && acknowledgedIds.has(event.comment.id);
      if (!reflected) {
        const reason = outcome?.reason ?? "record is not authoritative in the current roadmap state";
        console.error(`Rejecting roadmap record in comment ${event.comment.id}: ${reason}`);
      }
      await setLifecycleReaction(owner, repo, event.comment.id, reflected ? "rocket" : "confused", token);
    }

    try {
      await reconcileReactions(
        owner,
        repo,
        acknowledgedCommentIds(before.updates, before.current, before.outcomes),
        acknowledgedCommentIds(updates, current, outcomes),
        updates,
        outcomes,
        event.comment.id,
        event.action === "deleted" ? event.comment.id : null,
        token,
      );
    } catch (error) {
      console.error(`Roadmap projection committed but reaction reconciliation failed: ${error.message}`);
      throw error;
    }

    if (!tracksLifecycle) return { ...changed, updates: updates.length };
    return { ...changed, updates: updates.length, acknowledged: reflected ? "accepted" : "rejected" };
  } catch (error) {
    if (tracksLifecycle && !projectionCommitted) {
      try {
        await setLifecycleReaction(owner, repo, event.comment.id, "confused", token);
      } catch (reactionError) {
        console.error(`Could not mark comment ${event.comment.id} rejected: ${reactionError.message}`);
      }
    }
    throw error;
  }
}

async function main() {
  const eventPath = process.env.GITHUB_EVENT_PATH;
  const issueNumber = Number(process.env.PHOENIX_ROADMAP_ISSUE_NUMBER);
  const token = process.env.GITHUB_TOKEN;
  if (!eventPath || !Number.isSafeInteger(issueNumber) || issueNumber <= 0 || !token) {
    throw new Error("GITHUB_EVENT_PATH, GITHUB_TOKEN, and PHOENIX_ROADMAP_ISSUE_NUMBER are required");
  }
  const event = JSON.parse(await fs.readFile(eventPath, "utf8"));
  console.log(JSON.stringify(await run({ event, configuredIssueNumber: issueNumber, token })));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
