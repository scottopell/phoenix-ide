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

function reduceUpdates(comments, bounded) {
  const latest = new Map();
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
        } else {
          console.warn(`Ignoring retirement in comment ${comment.id}: it must supersede the same author's current source`);
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
    } catch (error) {
      console.warn(`Ignoring invalid roadmap record in comment ${comment.id}: ${error.message}`);
    }
  }

  const orderedUpdates = [...latest.values()].sort((left, right) => {
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
  return bounded ? orderedUpdates.slice(0, MAX_WORKSTREAMS) : orderedUpdates;
}

export function updatesFromComments(comments) {
  return reduceUpdates(comments, true);
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

function commentSnapshot(comments) {
  return comments
    .filter((comment) => Number.isSafeInteger(comment.id) && TRUSTED_ASSOCIATIONS.has(comment.author_association))
    .map((comment) => `${comment.id}:${comment.updated_at ?? comment.created_at}`)
    .join("|");
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

async function setLifecycleReaction(owner, repo, commentId, content, token) {
  const path = `/repos/${owner}/${repo}/issues/comments/${commentId}/reactions`;
  await githubRequest(path, token, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ content }),
  });
  const reactions = [];
  for (let page = 1; ; page += 1) {
    const batch = await githubRequest(`${path}?per_page=100&page=${page}`, token);
    reactions.push(...batch);
    if (batch.length < 100) break;
  }
  for (const reaction of reactions) {
    if (
      reaction.user?.login === "github-actions[bot]" &&
      LIFECYCLE_REACTIONS.has(reaction.content) &&
      reaction.content !== content
    ) {
      await githubRequest(`${path}/${reaction.id}`, token, { method: "DELETE" });
    }
  }
}

function createdRecordAccepted(comment, comments, renderedUpdates) {
  const prefix = comments.filter((candidate) => candidate.id <= comment.id);
  const record = roadmapPayload(comment.body);
  if (record?.recordType === "update") {
    return renderedUpdates.some((update) => update.source.id === comment.id);
  }
  if (record?.recordType === "retirement") {
    try {
      const retirement = validateRetirement(JSON.parse(record.payload));
      const before = reduceUpdates(prefix.filter((candidate) => candidate.id !== comment.id), false);
      const current = before.find((update) => update.workstream === retirement.workstream);
      const author = comment.user?.login ?? "unknown";
      return current === undefined ||
        (current.source.id <= retirement.supersedes_comment_id && current.source.author === author);
    } catch {
      return false;
    }
  }
  return false;
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
  const acknowledgesCreation = event.action === "created" && roadmapPayload(event.comment.body) !== null;

  try {
    if (acknowledgesCreation) await setLifecycleReaction(owner, repo, event.comment.id, "eyes", token);
    let comments;
    let updates;
    let changed;
    let stable = false;
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      comments = await listComments(owner, repo, configuredIssueNumber, token);
      const trustedCommentIds = comments
        .filter((comment) => Number.isSafeInteger(comment.id) && TRUSTED_ASSOCIATIONS.has(comment.author_association))
        .map((comment) => comment.id);
      const snapshotThroughCommentId = trustedCommentIds.length === 0 ? 0 : Math.max(...trustedCommentIds);
      updates = updatesFromComments(comments);
      const body = renderRoadmap(updates, snapshotThroughCommentId);
      changed = await replaceIssueBody(owner, repo, configuredIssueNumber, body, token);
      try {
        const after = await listComments(owner, repo, configuredIssueNumber, token);
        if (commentSnapshot(comments) === commentSnapshot(after)) {
          stable = true;
          break;
        }
      } catch (error) {
        console.warn(`Roadmap verification attempt ${attempt} failed; rebuilding from a fresh snapshot: ${error.message}`);
      }
    }
    if (!stable) {
      console.warn("Roadmap comments kept changing through 3 projection attempts; later uncancelled events will project newer comments");
    }

    if (!acknowledgesCreation) return { ...changed, updates: updates.length };
    const accepted = createdRecordAccepted(event.comment, comments, updates);
    if (!accepted) {
      console.error(`Rejecting roadmap record in comment ${event.comment.id}: it was invalid or superseded before application`);
    }
    await setLifecycleReaction(owner, repo, event.comment.id, accepted ? "rocket" : "confused", token);
    return { ...changed, updates: updates.length, acknowledged: accepted ? "accepted" : "rejected" };
  } catch (error) {
    if (acknowledgesCreation) {
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
