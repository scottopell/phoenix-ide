#!/usr/bin/env node

import fs from "node:fs/promises";
import process from "node:process";
import { pathToFileURL } from "node:url";

export const PROJECTION_START = "<!-- phoenix-roadmap:projection:start -->";
export const PROJECTION_END = "<!-- phoenix-roadmap:projection:end -->";
const REDUCED_THROUGH = /<!-- phoenix-roadmap:reduced-through:(\d+) -->/;

const UPDATE_FENCE = /```phoenix-roadmap-update\s*\n([\s\S]*?)\n```/g;
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
  return value.trim();
}

function optionalText(value, field) {
  if (value === undefined || value === null || value === "") return "";
  if (typeof value !== "string") throw new Error(`${field} must be a string`);
  if (value.includes(PROJECTION_START) || value.includes(PROJECTION_END)) {
    throw new Error(`${field} contains a reserved projection marker`);
  }
  return value.trim();
}

function validateEvidence(value) {
  if (!Array.isArray(value) || value.length === 0) {
    throw new Error("evidence must contain at least one link");
  }
  return value.map((item, index) => {
    if (item === null || typeof item !== "object" || Array.isArray(item)) {
      throw new Error(`evidence[${index}] must be an object`);
    }
    const label = requiredLine(item.label, `evidence[${index}].label`);
    const url = requiredLine(item.url, `evidence[${index}].url`);
    const parsed = new URL(url);
    if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
      throw new Error(`evidence[${index}].url must use http or https`);
    }
    return { label, url: parsed.toString() };
  });
}

export function validateUpdate(value) {
  if (value === null || typeof value !== "object" || Array.isArray(value)) {
    throw new Error("update must be an object");
  }
  if (value.kind !== "workstream-update" || value.version !== 1) {
    throw new Error("update must have kind workstream-update and version 1");
  }

  const workstream = requiredLine(value.workstream, "workstream");
  if (!/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(workstream)) {
    throw new Error("workstream must be a lowercase kebab-case identifier");
  }
  const section = requiredLine(value.section, "section");
  if (!VALID_SECTIONS.has(section)) throw new Error(`unknown section: ${section}`);
  const order = value.order ?? 100;
  if (!Number.isSafeInteger(order) || order < 0) {
    throw new Error("order must be a non-negative integer");
  }
  if (!Array.isArray(value.blocked_by) || value.blocked_by.some((item) => typeof item !== "string" || item.trim() === "" || /[\r\n]/.test(item))) {
    throw new Error("blocked_by must be an array of non-empty single-line strings");
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

export function updatesFromComments(comments, triggerCommentId) {
  const latest = new Map();
  const ordered = [...comments]
    .filter(
      (comment) =>
        Number.isSafeInteger(comment.id) &&
        comment.id <= triggerCommentId &&
        comment.created_at === comment.updated_at &&
        TRUSTED_ASSOCIATIONS.has(comment.author_association),
    )
    .sort((left, right) => left.id - right.id);

  for (const comment of ordered) {
    for (const match of String(comment.body ?? "").matchAll(UPDATE_FENCE)) {
      try {
        const update = validateUpdate(JSON.parse(match[1]));
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
        console.warn(`Ignoring invalid roadmap update in comment ${comment.id}: ${error.message}`);
      }
    }
  }

  return [...latest.values()].sort((left, right) => {
    const sectionOrder =
      SECTION_ORDER.findIndex(([key]) => key === left.section) -
      SECTION_ORDER.findIndex(([key]) => key === right.section);
    if (sectionOrder !== 0) return sectionOrder;
    if (left.order !== right.order) return left.order - right.order;
    return left.workstream < right.workstream ? -1 : left.workstream > right.workstream ? 1 : 0;
  });
}

function markdownText(value) {
  return value.replaceAll("\\", "\\\\").replace(/([`*_{}\[\]<>])/g, "\\$1");
}

function renderEvidence(evidence) {
  return evidence.map(({ label, url }) => `[${markdownText(label)}](${url})`).join(" · ");
}

function renderContext(context) {
  if (!context) return "";
  return `\n\nAgent context:\n\n${context.split("\n").map((line) => `> ${line}`).join("\n")}`;
}

function renderUpdate(update, open) {
  const blockers = update.blocked_by.length === 0 ? "None" : update.blocked_by.map(markdownText).join(" · ");
  const sourceLabel = `@${update.source.author} update`;
  return `<details${open ? " open" : ""}>\n<summary><strong>${markdownText(update.title)}</strong> — ${markdownText(update.state)}</summary>\n\nOwner: ${markdownText(update.owner)}  \nBlocked by: ${blockers}  \nNext: ${markdownText(update.next)}  \nEvidence: ${renderEvidence(update.evidence)}  \nSource: [${markdownText(sourceLabel)}](${update.source.url})${renderContext(update.context)}\n\n</details>`;
}

export function renderProjection(updates, triggerComment) {
  const lines = [
    PROJECTION_START,
    `<!-- phoenix-roadmap:reduced-through:${triggerComment.id} -->`,
    "",
    "## Current roadmap",
    "",
    `_Reduced through agent comment at ${triggerComment.created_at}_`,
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
  lines.push("", PROJECTION_END);
  return lines.join("\n");
}

export function replaceProjection(body, projection) {
  const current = String(body ?? "");
  const start = current.indexOf(PROJECTION_START);
  const end = current.indexOf(PROJECTION_END);
  if (start === -1 && end === -1) {
    return current.trim() === "" ? projection : `${projection}\n\n${current}`;
  }
  if (start === -1 || end === -1 || end < start) {
    throw new Error("roadmap Issue body has unmatched projection markers");
  }
  const after = end + PROJECTION_END.length;
  return `${current.slice(0, start)}${projection}${current.slice(after)}`;
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

async function replaceIssueProjection(owner, repo, issueNumber, projection, triggerCommentId, token) {
  const path = `/repos/${owner}/${repo}/issues/${issueNumber}`;
  for (let attempt = 1; attempt <= 3; attempt += 1) {
    const response = await githubResponse(path, token);
    if (!response.ok) throw new Error(`GitHub GET ${path}: ${response.status} ${await response.text()}`);
    const etag = response.headers.get("etag");
    if (!etag) throw new Error("GitHub Issue response lacks an ETag required for safe replacement");
    const issue = await response.json();
    const alreadyReducedThrough = Number(issue.body?.match(REDUCED_THROUGH)?.[1] ?? 0);
    if (alreadyReducedThrough > triggerCommentId) {
      return { skipped: "a newer agent comment is already projected" };
    }
    const body = replaceProjection(issue.body, projection);
    if (body === issue.body) return { changed: false };

    const patched = await githubResponse(path, token, {
      method: "PATCH",
      headers: { "Content-Type": "application/json", "If-Match": etag },
      body: JSON.stringify({ body }),
    });
    if (patched.status === 412) continue;
    if (!patched.ok) throw new Error(`GitHub PATCH ${path}: ${patched.status} ${await patched.text()}`);
    return { changed: true };
  }
  throw new Error("roadmap Issue changed during all three conditional replacement attempts");
}

async function listComments(owner, repo, issueNumber, token) {
  const comments = [];
  for (let page = 1; ; page += 1) {
    const batch = await githubRequest(`/repos/${owner}/${repo}/issues/${issueNumber}/comments?per_page=100&page=${page}`, token);
    comments.push(...batch);
    if (batch.length < 100) return comments;
  }
}

export async function run({ event, configuredIssueNumber, token }) {
  if (event.action !== "created" || event.issue?.pull_request) return { skipped: "not a created Issue comment" };
  if (event.issue?.number !== configuredIssueNumber) return { skipped: "not the configured roadmap Issue" };
  if (!Number.isSafeInteger(event.comment?.id) || !event.comment?.created_at) throw new Error("event lacks an immutable triggering comment identity");
  if (!TRUSTED_ASSOCIATIONS.has(event.comment.author_association)) {
    return { skipped: "triggering author is not trusted" };
  }
  if (updatesFromComments([event.comment], event.comment.id).length === 0) {
    return { skipped: "triggering comment contains no valid roadmap update" };
  }

  const [owner, repo] = event.repository.full_name.split("/");
  const comments = await listComments(owner, repo, configuredIssueNumber, token);
  const updates = updatesFromComments(comments, event.comment.id);
  const projection = renderProjection(updates, event.comment);
  return {
    ...(await replaceIssueProjection(owner, repo, configuredIssueNumber, projection, event.comment.id, token)),
    updates: updates.length,
  };
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
