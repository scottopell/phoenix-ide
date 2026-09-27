#!/usr/bin/env node

import fs from "node:fs/promises";
import process from "node:process";
import { pathToFileURL } from "node:url";

export const PROJECTION_START = "<!-- phoenix-roadmap:projection:start -->";
export const PROJECTION_END = "<!-- phoenix-roadmap:projection:end -->";
export const RECORD_FENCE = "phoenix-roadmap";
export const STAGES = ["implemented", "qualified", "merged", "released", "deployed", "accepted"];

const VERSION = 2;
const MAX_RECORD_BYTES = 2_000;
const MAX_ISSUE_BODY_BYTES = 65_536;
const MAX_LIVE_OUTCOMES = 40;
const MAX_PULL_FETCHES = 60;
const STALE_AFTER_HOURS = 72;
const RECENTLY_RETIRED_DAYS = 14;
const MAX_RENDERED_REJECTIONS = 20;
const TRUSTED_ASSOCIATIONS = new Set(["OWNER", "MEMBER", "COLLABORATOR"]);
const ROLES = new Set(["coordinator", "worker", "user"]);
const CLEARERS = new Set(["user", "coordinator", "owner", "external"]);
const ID_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const HARNESS_PATTERN = /^[a-z0-9]+(?:-[a-z0-9]+)*(?:@[a-z0-9]+(?:[.-][a-z0-9]+)*)?$/;
const SHA_PATTERN = /^[0-9a-f]{7,40}$/;

const PERMISSIONS = {
  outcome: ["coordinator"],
  "outcome-retire": ["coordinator"],
  milestone: ["coordinator"],
  "milestone-retire": ["coordinator"],
  gate: ["coordinator", "worker"],
  "gate-clear": ["coordinator", "worker"],
  decision: ["coordinator", "user"],
  evidence: ["coordinator", "worker", "user"],
  status: ["coordinator", "worker"],
};

class Rejection extends Error {}

function reject(message) {
  throw new Rejection(message);
}

function isObject(value) {
  return value !== null && typeof value === "object" && !Array.isArray(value);
}

function onlyKeys(value, allowed, field) {
  for (const key of Object.keys(value)) {
    if (!allowed.includes(key)) reject(`${field} has unknown field ${key}`);
  }
}

function line(value, field, max = 200) {
  if (typeof value !== "string" || value.trim() === "" || /[\r\n]/.test(value)) {
    reject(`${field} must be a non-empty single-line string`);
  }
  const trimmed = value.trim();
  if (trimmed.length > max) reject(`${field} must be at most ${max} characters`);
  if (trimmed.includes(PROJECTION_START) || trimmed.includes(PROJECTION_END)) {
    reject(`${field} contains a reserved projection marker`);
  }
  return trimmed;
}

function optionalLine(value, field, max = 200) {
  return value === undefined ? undefined : line(value, field, max);
}

function id(value, field) {
  const text = line(value, field, 64);
  if (!ID_PATTERN.test(text)) reject(`${field} must be a lowercase kebab-case identifier`);
  return text;
}

function sha(value, field) {
  if (typeof value !== "string" || !SHA_PATTERN.test(value)) {
    reject(`${field} must be a 7-40 character lowercase hex commit SHA`);
  }
  return value;
}

function pullNumber(value, field) {
  if (!Number.isSafeInteger(value) || value <= 0) reject(`${field} must be a positive integer`);
  return value;
}

function url(value, field, { httpsOnly }) {
  const text = line(value, field, 500);
  let parsed;
  try {
    parsed = new URL(text);
  } catch {
    reject(`${field} must be an absolute URL`);
  }
  const allowed = httpsOnly ? ["https:"] : ["https:", "http:"];
  if (!allowed.includes(parsed.protocol)) reject(`${field} must use ${allowed.join(" or ")}`);
  return parsed.toString();
}

function list(value, field, min, max, item) {
  if (!Array.isArray(value) || value.length < min || value.length > max) {
    reject(`${field} must contain between ${min} and ${max} items`);
  }
  return value.map((entry, index) => item(entry, `${field}[${index}]`));
}

function handle(value, field) {
  const text = line(value, field, 64);
  if (!HARNESS_PATTERN.test(text)) reject(`${field} must look like phoenix@machine, codex, claude-code, or prod@machine`);
  return text;
}

function actor(value, coordinatorHarness) {
  if (!isObject(value)) reject("actor must be an object");
  onlyKeys(value, ["role", "harness"], "actor");
  if (!ROLES.has(value.role)) reject("actor.role must be coordinator, worker, or user");
  const harness = handle(value.harness, "actor.harness");
  if (value.role === "coordinator" && coordinatorHarness && harness !== coordinatorHarness) {
    reject(`coordinator records must come from ${coordinatorHarness}`);
  }
  return { role: value.role, harness };
}

function validateSubject(stage, value) {
  if (!isObject(value)) reject("subject must be an object");
  switch (stage) {
    case "implemented":
      onlyKeys(value, ["pr", "commit"], "subject");
      if (value.pr === undefined && value.commit === undefined) reject("implemented evidence needs subject.pr or subject.commit");
      return {
        ...(value.pr !== undefined && { pr: pullNumber(value.pr, "subject.pr") }),
        ...(value.commit !== undefined && { commit: sha(value.commit, "subject.commit") }),
      };
    case "qualified":
      onlyKeys(value, ["pr", "head"], "subject");
      return { pr: pullNumber(value.pr, "subject.pr"), head: sha(value.head, "subject.head") };
    case "merged":
      onlyKeys(value, ["pr", "commit"], "subject");
      return {
        commit: sha(value.commit, "subject.commit"),
        ...(value.pr !== undefined && { pr: pullNumber(value.pr, "subject.pr") }),
      };
    case "released":
      onlyKeys(value, ["release", "commit"], "subject");
      return { release: line(value.release, "subject.release", 100), commit: sha(value.commit, "subject.commit") };
    case "deployed":
      onlyKeys(value, ["target", "commit"], "subject");
      return { target: handle(value.target, "subject.target"), commit: sha(value.commit, "subject.commit") };
    case "accepted":
      onlyKeys(value, ["commit"], "subject");
      return value.commit === undefined ? {} : { commit: sha(value.commit, "subject.commit") };
    default:
      return reject(`unknown stage ${stage}`);
  }
}

function subjectKey(stage, subject) {
  switch (stage) {
    case "implemented":
      return subject.pr !== undefined ? `pr:${subject.pr}` : `commit:${subject.commit}`;
    case "qualified":
      return `pr:${subject.pr}@${subject.head}`;
    case "merged":
      return `commit:${subject.commit}`;
    case "released":
      return `release:${subject.release}`;
    case "deployed":
      return `target:${subject.target}`;
    default:
      return "";
  }
}

const VALIDATORS = {
  outcome(value) {
    onlyKeys(value, ["kind", "version", "actor", "id", "title", "intent", "acceptance", "owner", "order", "surfaces"], "outcome");
    if (!isObject(value.owner)) reject("owner must be an object");
    onlyKeys(value.owner, ["harness", "label"], "owner");
    const ownerHarness = handle(value.owner.harness, "owner.harness");
    const order = value.order ?? 100;
    if (!Number.isSafeInteger(order) || order < 0) reject("order must be a non-negative integer");
    return {
      id: id(value.id, "id"),
      title: line(value.title, "title", 120),
      intent: line(value.intent, "intent", 300),
      acceptance: list(value.acceptance, "acceptance", 1, 5, (entry, field) => line(entry, field)),
      owner: { harness: ownerHarness, label: optionalLine(value.owner.label, "owner.label", 80) },
      order,
      surfaces: list(value.surfaces, "surfaces", 1, 4, (entry, field) => id(entry, field)),
    };
  },
  "outcome-retire"(value) {
    onlyKeys(value, ["kind", "version", "actor", "outcome", "reason", "note"], "outcome-retire");
    if (!["delivered", "dropped"].includes(value.reason)) reject("reason must be delivered or dropped");
    return { outcome: id(value.outcome, "outcome"), reason: value.reason, note: optionalLine(value.note, "note") };
  },
  milestone(value) {
    onlyKeys(value, ["kind", "version", "actor", "id", "title", "order", "required", "optional", "decision"], "milestone");
    const order = value.order ?? 100;
    if (!Number.isSafeInteger(order) || order < 0) reject("order must be a non-negative integer");
    return {
      id: id(value.id, "id"),
      title: line(value.title, "title", 120),
      order,
      required: list(value.required, "required", 1, 12, (entry, field) => {
        if (!isObject(entry)) reject(`${field} must be an object`);
        onlyKeys(entry, ["outcome", "surface", "stage"], field);
        if (!STAGES.includes(entry.stage)) reject(`${field}.stage must be one of ${STAGES.join(", ")}`);
        return { outcome: id(entry.outcome, `${field}.outcome`), surface: id(entry.surface, `${field}.surface`), stage: entry.stage };
      }),
      optional: list(value.optional ?? [], "optional", 0, 12, (entry, field) => id(entry, field)),
      decision: value.decision === undefined ? undefined : id(value.decision, "decision"),
    };
  },
  "milestone-retire"(value) {
    onlyKeys(value, ["kind", "version", "actor", "milestone", "reason", "note"], "milestone-retire");
    if (!["reached", "abandoned"].includes(value.reason)) reject("reason must be reached or abandoned");
    return { milestone: id(value.milestone, "milestone"), reason: value.reason, note: optionalLine(value.note, "note") };
  },
  gate(value) {
    onlyKeys(value, ["kind", "version", "actor", "id", "blocks", "clearer", "condition"], "gate");
    if (!isObject(value.blocks)) reject("blocks must be an object");
    onlyKeys(value.blocks, ["outcome", "milestone"], "blocks");
    const targets = Object.keys(value.blocks);
    if (targets.length !== 1) reject("blocks must name exactly one outcome or milestone");
    if (!CLEARERS.has(value.clearer)) reject("clearer must be user, coordinator, owner, or external");
    return {
      id: id(value.id, "id"),
      blocks: { type: targets[0], id: id(value.blocks[targets[0]], `blocks.${targets[0]}`) },
      clearer: value.clearer,
      condition: line(value.condition, "condition"),
    };
  },
  "gate-clear"(value) {
    onlyKeys(value, ["kind", "version", "actor", "gate", "evidence", "note"], "gate-clear");
    return {
      gate: id(value.gate, "gate"),
      evidence: url(value.evidence, "evidence", { httpsOnly: true }),
      note: optionalLine(value.note, "note"),
    };
  },
  decision(value, recordActor) {
    onlyKeys(value, ["kind", "version", "actor", "id", "statement", "scope", "supersedes", "clears", "quote", "source"], "decision");
    const quote = optionalLine(value.quote, "quote", 300);
    if (recordActor.role === "user" && quote === undefined) reject("user decisions must quote the user's words");
    return {
      id: id(value.id, "id"),
      statement: line(value.statement, "statement", 300),
      scope: list(value.scope, "scope", 1, 8, (entry, field) => id(entry, field)),
      supersedes: list(value.supersedes ?? [], "supersedes", 0, 5, (entry, field) => id(entry, field)),
      clears: list(value.clears ?? [], "clears", 0, 5, (entry, field) => id(entry, field)),
      quote,
      source: value.source === undefined ? undefined : url(value.source, "source", { httpsOnly: false }),
    };
  },
  evidence(value, recordActor) {
    onlyKeys(value, ["kind", "version", "actor", "outcome", "surface", "stage", "result", "subject", "url", "note"], "evidence");
    if (!STAGES.includes(value.stage)) reject(`stage must be one of ${STAGES.join(", ")}`);
    if (!["pass", "fail"].includes(value.result)) reject("result must be pass or fail");
    if (value.stage === "accepted" && recordActor.role === "worker") reject("accepted evidence comes from the user or the coordinator");
    if (value.stage !== "accepted" && recordActor.role === "user") reject("user evidence is limited to the accepted stage");
    const subject = validateSubject(value.stage, value.subject ?? {});
    return {
      outcome: id(value.outcome, "outcome"),
      surface: id(value.surface, "surface"),
      stage: value.stage,
      result: value.result,
      subject,
      url: url(value.url, "url", { httpsOnly: true }),
      note: optionalLine(value.note, "note"),
    };
  },
  status(value) {
    onlyKeys(value, ["kind", "version", "actor", "outcome", "next", "pointers", "note"], "status");
    return {
      outcome: id(value.outcome, "outcome"),
      next: line(value.next),
      pointers: list(value.pointers ?? [], "pointers", 0, 5, (entry, field) => {
        if (!isObject(entry)) reject(`${field} must be an object`);
        onlyKeys(entry, ["label", "url", "harness"], field);
        const harness = entry.harness === undefined ? undefined : handle(entry.harness, `${field}.harness`);
        return { label: line(entry.label, `${field}.label`, 80), url: url(entry.url, `${field}.url`, { httpsOnly: false }), harness };
      }),
      note: optionalLine(value.note, "note"),
    };
  },
};

export function validateRecord(value, { coordinatorHarness } = {}) {
  if (!isObject(value)) reject("record must be a JSON object");
  if (Buffer.byteLength(JSON.stringify(value), "utf8") > MAX_RECORD_BYTES) {
    reject(`record must be at most ${MAX_RECORD_BYTES} UTF-8 bytes`);
  }
  if (value.version !== VERSION) reject(`version must be ${VERSION}`);
  if (!Object.hasOwn(PERMISSIONS, value.kind)) reject(`unknown kind ${String(value.kind)}`);
  const recordActor = actor(value.actor, coordinatorHarness);
  if (!PERMISSIONS[value.kind].includes(recordActor.role)) {
    reject(`${recordActor.role} may not post ${value.kind} records`);
  }
  return { kind: value.kind, actor: recordActor, ...VALIDATORS[value.kind](value, recordActor) };
}

export function parseRecordComment(body) {
  const text = String(body ?? "");
  const exact = text.match(/^```phoenix-roadmap\r?\n([\s\S]*?)\r?\n```\s*$/);
  if (exact) return { payload: exact[1] };
  if (/```phoenix-roadmap-(update|retirement)\b/.test(text)) {
    return { error: "v1 roadmap records are no longer accepted; see specs/roadmap/requirements.md" };
  }
  if (/```phoenix-roadmap\b/.test(text)) {
    return { error: "a record comment must consist only of one phoenix-roadmap fence" };
  }
  return null;
}

function isTrusted(comment) {
  return Number.isSafeInteger(comment.id) && TRUSTED_ASSOCIATIONS.has(comment.author_association);
}

function sourceOf(comment) {
  return { id: comment.id, url: comment.html_url, created_at: comment.created_at };
}

function newState() {
  return {
    ids: new Map(),
    outcomes: new Map(),
    milestones: new Map(),
    gates: new Map(),
    decisions: new Map(),
    evidence: new Map(),
    status: new Map(),
    checkins: new Map(),
    conflicts: [],
    mixedResults: new Set(),
    rejections: [],
    accepted: new Set(),
  };
}

function claimId(state, kind, recordId) {
  const existing = state.ids.get(recordId);
  if (existing !== undefined && existing !== kind) reject(`id ${recordId} is already used by a ${existing}`);
}

function liveOutcome(state, outcomeId, field = "outcome") {
  const outcome = state.outcomes.get(outcomeId);
  if (!outcome) reject(`${field} ${outcomeId} does not exist`);
  if (outcome.retired) reject(`${field} ${outcomeId} is retired`);
  return outcome;
}

function liveMilestone(state, milestoneId) {
  const milestone = state.milestones.get(milestoneId);
  if (!milestone) reject(`milestone ${milestoneId} does not exist`);
  if (milestone.retired) reject(`milestone ${milestoneId} is retired`);
  return milestone;
}

function openGate(state, gateId) {
  const gate = state.gates.get(gateId);
  if (!gate) reject(`gate ${gateId} does not exist`);
  if (gate.cleared) reject(`gate ${gateId} is already cleared`);
  return gate;
}

function currentDecision(state, decisionId) {
  const decision = state.decisions.get(decisionId);
  return decision && decision.supersededBy.length === 0 ? decision : undefined;
}

function checkIn(state, outcomeId, source) {
  const previous = state.checkins.get(outcomeId);
  if (!previous || previous.id < source.id) state.checkins.set(outcomeId, source);
}

function canonicalRequired(required) {
  return JSON.stringify([...required].map((entry) => `${entry.outcome}/${entry.surface}/${entry.stage}`).sort());
}

const APPLY = {
  outcome(state, record, source) {
    claimId(state, "outcome", record.id);
    const existing = state.outcomes.get(record.id);
    if (existing?.retired) reject(`outcome ${record.id} is retired`);
    if (!existing) {
      const live = [...state.outcomes.values()].filter((outcome) => !outcome.retired).length;
      if (live >= MAX_LIVE_OUTCOMES) reject(`at most ${MAX_LIVE_OUTCOMES} live outcomes; retire one first`);
    }
    state.ids.set(record.id, "outcome");
    state.outcomes.set(record.id, { ...record, source, created: existing?.created ?? source });
  },
  "outcome-retire"(state, record, source) {
    const outcome = liveOutcome(state, record.outcome);
    outcome.retired = { reason: record.reason, note: record.note, source };
  },
  milestone(state, record, source) {
    claimId(state, "milestone", record.id);
    const existing = state.milestones.get(record.id);
    if (existing?.retired) reject(`milestone ${record.id} is retired`);
    const optional = new Set(record.optional);
    for (const entry of record.required) {
      const outcome = liveOutcome(state, entry.outcome, "required outcome");
      if (!outcome.surfaces.includes(entry.surface)) reject(`outcome ${entry.outcome} has no surface ${entry.surface}`);
      if (optional.has(entry.outcome)) reject(`outcome ${entry.outcome} cannot be both required and optional`);
    }
    for (const outcomeId of record.optional) liveOutcome(state, outcomeId, "optional outcome");
    if (existing && canonicalRequired(existing.required) !== canonicalRequired(record.required)) {
      const decision = record.decision && currentDecision(state, record.decision);
      if (!decision || !decision.scope.includes(record.id)) {
        reject(`changing milestone ${record.id} requirements needs a current decision scoped to it`);
      }
    }
    state.ids.set(record.id, "milestone");
    state.milestones.set(record.id, { ...record, source });
  },
  "milestone-retire"(state, record, source) {
    const milestone = liveMilestone(state, record.milestone);
    milestone.retired = { reason: record.reason, note: record.note, source };
  },
  gate(state, record, source) {
    if (state.ids.has(record.id)) reject(`id ${record.id} is already used; gate ids are single-use`);
    if (record.blocks.type === "milestone") {
      if (record.actor.role !== "coordinator") reject("only the coordinator may gate a milestone");
      liveMilestone(state, record.blocks.id);
    } else {
      liveOutcome(state, record.blocks.id);
    }
    state.ids.set(record.id, "gate");
    state.gates.set(record.id, { ...record, source });
    if (record.blocks.type === "outcome") checkIn(state, record.blocks.id, source);
  },
  "gate-clear"(state, record, source) {
    const gate = openGate(state, record.gate);
    if (gate.clearer === "user") reject(`gate ${gate.id} is cleared only by a user decision`);
    if (gate.clearer === "coordinator" && record.actor.role !== "coordinator") {
      reject(`gate ${gate.id} is cleared only by the coordinator`);
    }
    gate.cleared = { evidence: record.evidence, note: record.note, source };
  },
  decision(state, record, source) {
    if (state.ids.has(record.id)) reject(`id ${record.id} is already used; decisions are immutable`);
    for (const scopeId of record.scope) {
      if (!state.ids.has(scopeId)) reject(`scope ${scopeId} does not exist`);
    }
    const superseded = record.supersedes.map((decisionId) => {
      const decision = state.decisions.get(decisionId);
      if (!decision) reject(`supersedes ${decisionId} does not exist`);
      return decision;
    });
    const gates = record.clears.map((gateId) => {
      const gate = openGate(state, gateId);
      if (gate.clearer === "user" && record.actor.role !== "user") reject(`gate ${gateId} is cleared only by a user decision`);
      return gate;
    });
    state.ids.set(record.id, "decision");
    state.decisions.set(record.id, { ...record, source, supersededBy: [] });
    for (const decision of superseded) {
      if (decision.supersededBy.length > 0) {
        state.conflicts.push({
          message: `decision ${decision.id} is superseded by both ${decision.supersededBy.join(", ")} and ${record.id}`,
          source,
        });
      }
      decision.supersededBy.push(record.id);
    }
    for (const gate of gates) gate.cleared = { decision: record.id, source };
  },
  evidence(state, record, source) {
    const outcome = liveOutcome(state, record.outcome);
    if (!outcome.surfaces.includes(record.surface)) reject(`outcome ${record.outcome} has no surface ${record.surface}`);
    const key = `${record.outcome}|${record.surface}|${record.stage}|${subjectKey(record.stage, record.subject)}`;
    const entries = state.evidence.get(key) ?? [];
    entries.push({ ...record, source });
    state.evidence.set(key, entries);
    if (record.stage === "qualified" && !state.mixedResults.has(key) && new Set(entries.map((entry) => entry.result)).size > 1) {
      state.mixedResults.add(key);
      state.conflicts.push({
        message: `${outcome.title}: both pass and fail recorded for ${record.surface} qualification of PR #${record.subject.pr} at ${record.subject.head.slice(0, 7)}`,
        source,
      });
    }
    checkIn(state, record.outcome, source);
  },
  status(state, record, source) {
    const outcome = liveOutcome(state, record.outcome);
    if (record.actor.role === "worker" && record.actor.harness !== outcome.owner.harness) {
      state.conflicts.push({
        message: `${outcome.title}: status from ${record.actor.harness}, but the accountable owner is ${outcome.owner.harness}`,
        source,
      });
    }
    state.status.set(record.outcome, { ...record, source });
    checkIn(state, record.outcome, source);
  },
};

export function reduceComments(comments, { coordinatorHarness } = {}) {
  const state = newState();
  const ordered = comments.filter(isTrusted).sort((left, right) => left.id - right.id);
  for (const comment of ordered) {
    const parsed = parseRecordComment(comment.body);
    if (!parsed) continue;
    const source = sourceOf(comment);
    try {
      if (parsed.error) reject(parsed.error);
      if (comment.updated_at && comment.created_at && comment.updated_at !== comment.created_at) {
        reject("edited records are ignored; post a new record instead");
      }
      let value;
      try {
        value = JSON.parse(parsed.payload);
      } catch (error) {
        reject(`invalid JSON: ${error.message}`);
      }
      const record = validateRecord(value, { coordinatorHarness });
      APPLY[record.kind](state, record, source);
      state.accepted.add(comment.id);
    } catch (error) {
      if (!(error instanceof Rejection)) throw error;
      state.rejections.push({ ...source, reason: error.message });
    }
  }
  return state;
}

function pullNumbersIn(state) {
  const numbers = new Set();
  for (const entries of state.evidence.values()) {
    for (const entry of entries) {
      if (entry.subject.pr !== undefined && !state.outcomes.get(entry.outcome)?.retired) numbers.add(entry.subject.pr);
    }
  }
  return [...numbers].sort((left, right) => left - right).slice(0, MAX_PULL_FETCHES);
}

function short(commit) {
  return commit.slice(0, 7);
}

function latest(entries) {
  return entries.reduce((best, entry) => (best === undefined || entry.source.id > best.source.id ? entry : best), undefined);
}

export function surfaceDelivery(state, outcomeId, surface, pulls) {
  const found = new Map();
  const notes = [];
  const prefix = `${outcomeId}|${surface}|`;
  for (const [key, entries] of state.evidence) {
    if (!key.startsWith(prefix)) continue;
    const entry = latest(entries);
    if (entry.result !== "pass") {
      if (entry.stage === "qualified") {
        const pull = pulls.get(entry.subject.pr);
        if (!pull || pull.head === entry.subject.head) notes.push(`✗ qualification failed at ${short(entry.subject.head)}`);
      }
      continue;
    }
    let detail;
    if (entry.stage === "qualified") {
      const pull = pulls.get(entry.subject.pr);
      if (pull && pull.head !== entry.subject.head) {
        notes.push(`qualified at old head ${short(entry.subject.head)}; PR #${entry.subject.pr} is now ${short(pull.head)}`);
        continue;
      }
      detail = `#${entry.subject.pr}@${short(entry.subject.head)}${pull ? "" : " (head unverified)"}`;
    } else if (entry.stage === "deployed") {
      detail = `${entry.subject.target}@${short(entry.subject.commit)}`;
    } else if (entry.stage === "released") {
      detail = entry.subject.release;
    } else if (entry.stage === "merged") {
      detail = short(entry.subject.commit);
    } else if (entry.stage === "implemented") {
      detail = entry.subject.pr !== undefined ? `#${entry.subject.pr}` : short(entry.subject.commit);
    } else {
      detail = "";
    }
    const current = found.get(entry.stage);
    if (!current || current.id < entry.source.id) found.set(entry.stage, { detail, id: entry.source.id });
    if (entry.subject.pr !== undefined) {
      const pull = pulls.get(entry.subject.pr);
      if (pull?.merged && pull.mergeCommit && !found.has("merged")) {
        found.set("merged", { detail: `#${entry.subject.pr}→${short(pull.mergeCommit)}`, id: 0 });
      }
    }
  }
  let highest = -1;
  for (const stage of found.keys()) highest = Math.max(highest, STAGES.indexOf(stage));
  return {
    stageIndex: highest,
    stage: highest < 0 ? undefined : STAGES[highest],
    detail: highest < 0 ? undefined : found.get(STAGES[highest]).detail,
    notes,
  };
}

function markdownText(value) {
  return String(value).replaceAll("\\", "\\\\").replace(/([`*_{}\[\]<>|])/g, "\\$1");
}

function link(label, href) {
  return `[${markdownText(label)}](${href})`;
}

function age(from, now) {
  if (!from) return "never";
  const hours = (now.getTime() - new Date(from).getTime()) / 3_600_000;
  const text = hours < 1 ? "<1h" : hours < 48 ? `${Math.floor(hours)}h` : `${Math.floor(hours / 24)}d`;
  return hours > STALE_AFTER_HOURS ? `${text} ⚠` : text;
}

function deliveryCell(state, outcome, pulls, wanted) {
  return outcome.surfaces
    .map((surface) => {
      const delivery = surfaceDelivery(state, outcome.id, surface, pulls);
      const required = wanted?.get(surface);
      const met = required === undefined ? "" : delivery.stageIndex >= STAGES.indexOf(required) ? " ✓" : ` → ${required}`;
      const value = delivery.stage ? `${delivery.stage}${delivery.detail ? ` ${delivery.detail}` : ""}` : "—";
      const notes = delivery.notes.length ? ` (${delivery.notes.join("; ")})` : "";
      return markdownText(`${surface}: ${value}${notes}`) + met;
    })
    .join("<br>");
}

function openGates(state, type, targetId) {
  return [...state.gates.values()].filter((gate) => !gate.cleared && gate.blocks.type === type && gate.blocks.id === targetId);
}

function gateText(gate) {
  return `${link(gate.id, gate.source.url)} ${markdownText(gate.condition)} (clears: ${gate.clearer})`;
}

function outcomeRow(state, outcome, pulls, now, wanted) {
  const status = state.status.get(outcome.id);
  const gates = openGates(state, "outcome", outcome.id).map(gateText).join("<br>") || "—";
  const owner = outcome.owner.label ? `${outcome.owner.label} (${outcome.owner.harness})` : outcome.owner.harness;
  const next = status
    ? `${markdownText(status.next)}${status.note ? ` — ${markdownText(status.note)}` : ""}${
        status.pointers.length ? `<br>${status.pointers.map((pointer) => link(pointer.harness ? `${pointer.label} (${pointer.harness})` : pointer.label, pointer.url)).join(" · ")}` : ""
      }`
    : "_no status yet_";
  const title = outcome.retired ? `${outcome.title} (${outcome.retired.reason})` : outcome.title;
  return `| ${link(title, outcome.source.url)} | ${markdownText(owner)} | ${deliveryCell(state, outcome, pulls, wanted)} | ${gates} | ${next} | ${age(state.checkins.get(outcome.id)?.created_at, now)} |`;
}

const TABLE_HEADER = ["| Outcome | Owner | Delivery | Open gates | Next | Checked in |", "|---|---|---|---|---|---|"];

function byOrder(left, right) {
  return left.order - right.order || (left.id < right.id ? -1 : left.id > right.id ? 1 : 0);
}

function renderMilestone(state, milestone, pulls, now) {
  const wantedByOutcome = new Map();
  for (const entry of milestone.required) {
    const wanted = wantedByOutcome.get(entry.outcome) ?? new Map();
    wanted.set(entry.surface, entry.stage);
    wantedByOutcome.set(entry.outcome, wanted);
  }
  const met = milestone.required.filter(
    (entry) => surfaceDelivery(state, entry.outcome, entry.surface, pulls).stageIndex >= STAGES.indexOf(entry.stage),
  ).length;
  const gates = [
    ...openGates(state, "milestone", milestone.id),
    ...[...wantedByOutcome.keys()].flatMap((outcomeId) => openGates(state, "outcome", outcomeId)),
  ];
  const lines = [`### ${link(milestone.title, milestone.source.url)} — ${met}/${milestone.required.length} required met`, ""];
  const milestoneGates = openGates(state, "milestone", milestone.id);
  if (milestoneGates.length) lines.push(`Milestone gates: ${milestoneGates.map(gateText).join(" · ")}`, "");
  if (!gates.length && met === milestone.required.length) lines.push("_All requirements met; awaiting coordinator retirement._", "");
  lines.push(...TABLE_HEADER);
  for (const outcomeId of wantedByOutcome.keys()) {
    lines.push(outcomeRow(state, state.outcomes.get(outcomeId), pulls, now, wantedByOutcome.get(outcomeId)));
  }
  if (milestone.optional.length) {
    const optional = milestone.optional.map((outcomeId) => markdownText(state.outcomes.get(outcomeId).title)).join(", ");
    lines.push("", `Optional, not blocking: ${optional}`);
  }
  const decisions = [...state.decisions.values()].filter(
    (decision) => decision.supersededBy.length === 0 && decision.scope.includes(milestone.id),
  );
  if (decisions.length) {
    lines.push("", `Decisions: ${decisions.map((decision) => `${link(decision.id, decision.source.url)} ${markdownText(decision.statement)}`).join(" · ")}`);
  }
  return lines;
}

export function renderRoadmap(state, { pulls = new Map(), pullsVerified = false, now = new Date(), snapshotThroughCommentId = 0 } = {}) {
  const lines = [
    "# Phoenix delivery roadmap",
    "",
    "Generated from structured records in this Issue's comments; do not edit this body. Protocol: `specs/roadmap/requirements.md`.",
    "",
    PROJECTION_START,
    `<!-- phoenix-roadmap:snapshot-through:${snapshotThroughCommentId} -->`,
    "",
    `_Rendered ${now.toISOString().slice(0, 16).replace("T", " ")} UTC · PR state ${pullsVerified ? "verified at render" : "not verified"}_`,
  ];

  const milestones = [...state.milestones.values()].filter((milestone) => !milestone.retired).sort(byOrder);
  const inMilestone = new Set(milestones.flatMap((milestone) => milestone.required.map((entry) => entry.outcome)));
  lines.push("", "## Milestones", "");
  if (!milestones.length) lines.push("_No active milestones._");
  for (const milestone of milestones) lines.push(...renderMilestone(state, milestone, pulls, now), "");

  const others = [...state.outcomes.values()].filter((outcome) => !outcome.retired && !inMilestone.has(outcome.id)).sort(byOrder);
  lines.push("", "## Outcomes outside milestones", "");
  if (others.length) {
    lines.push(...TABLE_HEADER, ...others.map((outcome) => outcomeRow(state, outcome, pulls, now)));
  } else {
    lines.push("_None._");
  }

  const orphanedRequirements = milestones.flatMap((milestone) =>
    milestone.required
      .filter((entry) => state.outcomes.get(entry.outcome)?.retired?.reason === "dropped")
      .map((entry) => ({ message: `${milestone.title} requires dropped outcome ${entry.outcome}`, source: milestone.source })),
  );
  const conflicts = [...state.conflicts, ...orphanedRequirements];
  lines.push("", "## Needs coordinator", "");
  lines.push(conflicts.length ? conflicts.map((conflict) => `- ${markdownText(conflict.message)} (${link("source", conflict.source.url)})`).join("\n") : "_Nothing._");

  const cutoff = now.getTime() - RECENTLY_RETIRED_DAYS * 86_400_000;
  const retired = [
    ...[...state.milestones.values()].filter((milestone) => milestone.retired).map((milestone) => ({ title: milestone.title, ...milestone.retired })),
    ...[...state.outcomes.values()].filter((outcome) => outcome.retired).map((outcome) => ({ title: outcome.title, ...outcome.retired })),
  ]
    .filter((entry) => new Date(entry.source.created_at).getTime() >= cutoff)
    .sort((left, right) => right.source.id - left.source.id);
  if (retired.length) {
    lines.push("", `## Retired in the last ${RECENTLY_RETIRED_DAYS} days`, "");
    lines.push(retired.map((entry) => `- ${markdownText(entry.title)} — ${entry.reason}${entry.note ? `: ${markdownText(entry.note)}` : ""} (${link("source", entry.source.url)})`).join("\n"));
  }

  const rejections = [...state.rejections].sort((left, right) => right.id - left.id).slice(0, MAX_RENDERED_REJECTIONS);
  lines.push("", "## Recent rejections", "");
  lines.push(rejections.length ? rejections.map((entry) => `- ${link(String(entry.id), entry.url)}: ${markdownText(entry.reason)}`).join("\n") : "_None._");

  lines.push("", PROJECTION_END);
  const body = lines.join("\n");
  if (Buffer.byteLength(body, "utf8") > MAX_ISSUE_BODY_BYTES) {
    throw new Error(`generated roadmap exceeds GitHub's ${MAX_ISSUE_BODY_BYTES}-byte Issue body limit`);
  }
  return body;
}

export function githubApi({ owner, repo, issueNumber, token }) {
  async function request(path, options = {}) {
    const response = await fetch(`https://api.github.com${path}`, {
      ...options,
      headers: {
        Accept: "application/vnd.github+json",
        Authorization: `Bearer ${token}`,
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "phoenix-roadmap-issue-reducer",
        ...(options.body && { "Content-Type": "application/json" }),
      },
    });
    if (!response.ok) throw new Error(`GitHub ${options.method ?? "GET"} ${path}: ${response.status} ${await response.text()}`);
    return response.status === 204 ? null : response.json();
  }
  async function paginate(path) {
    const items = [];
    for (let page = 1; ; page += 1) {
      const batch = await request(`${path}${path.includes("?") ? "&" : "?"}per_page=100&page=${page}`);
      items.push(...batch);
      if (batch.length < 100) return items;
    }
  }
  return {
    listComments: () => paginate(`/repos/${owner}/${repo}/issues/${issueNumber}/comments`),
    replaceBody: (body) => request(`/repos/${owner}/${repo}/issues/${issueNumber}`, { method: "PATCH", body: JSON.stringify({ body }) }),
    async getPull(number) {
      const pull = await request(`/repos/${owner}/${repo}/pulls/${number}`);
      return { head: pull.head.sha, merged: pull.merged === true, mergeCommit: pull.merge_commit_sha ?? undefined, state: pull.state };
    },
    async setReaction(commentId, content) {
      const path = `/repos/${owner}/${repo}/issues/comments/${commentId}/reactions`;
      await request(path, { method: "POST", body: JSON.stringify({ content }) });
      for (const reaction of await paginate(path)) {
        if (reaction.user?.login === "github-actions[bot]" && ["eyes", "rocket", "confused"].includes(reaction.content) && reaction.content !== content) {
          await request(`${path}/${reaction.id}`, { method: "DELETE" });
        }
      }
    },
  };
}

async function fetchPulls(api, numbers) {
  const pulls = new Map();
  let verified = true;
  for (const number of numbers) {
    try {
      pulls.set(number, await api.getPull(number));
    } catch (error) {
      verified = false;
      console.warn(`Could not read PR #${number}: ${error.message}`);
    }
  }
  return { pulls, verified };
}

function commentSnapshot(comments) {
  return comments
    .filter(isTrusted)
    .map((comment) => `${comment.id}:${comment.updated_at ?? comment.created_at}`)
    .join("|");
}

export async function run({ eventName, event, configuredIssueNumber, coordinatorHarness, api, now = () => new Date() }) {
  let acknowledge;
  if (eventName === "issue_comment") {
    if (!["created", "edited", "deleted"].includes(event.action) || event.issue?.pull_request) {
      return { skipped: "not a supported Issue comment event" };
    }
    if (event.issue?.number !== configuredIssueNumber) return { skipped: "not the configured roadmap Issue" };
    if (!TRUSTED_ASSOCIATIONS.has(event.comment?.author_association)) return { skipped: "triggering author is not trusted" };
    if (event.action === "created" && parseRecordComment(event.comment.body) !== null) acknowledge = event.comment.id;
  } else if (!["schedule", "workflow_dispatch"].includes(eventName)) {
    return { skipped: `unsupported event ${eventName}` };
  }

  try {
    if (acknowledge !== undefined) await api.setReaction(acknowledge, "eyes");
    let state;
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      const comments = await api.listComments();
      state = reduceComments(comments, { coordinatorHarness });
      const { pulls, verified } = await fetchPulls(api, pullNumbersIn(state));
      const trustedIds = comments.filter(isTrusted).map((comment) => comment.id);
      const snapshotThroughCommentId = trustedIds.length ? Math.max(...trustedIds) : 0;
      await api.replaceBody(renderRoadmap(state, { pulls, pullsVerified: verified, now: now(), snapshotThroughCommentId }));
      if (commentSnapshot(comments) === commentSnapshot(await api.listComments())) break;
      console.warn(`Roadmap comments changed during projection attempt ${attempt}; rebuilding`);
    }
    for (const rejection of state.rejections) console.warn(`Rejected comment ${rejection.id}: ${rejection.reason}`);
    const result = { outcomes: state.outcomes.size, rejections: state.rejections.length };
    if (acknowledge === undefined) return result;
    const accepted = state.accepted.has(acknowledge);
    await api.setReaction(acknowledge, accepted ? "rocket" : "confused");
    return { ...result, acknowledged: accepted ? "accepted" : "rejected" };
  } catch (error) {
    if (acknowledge !== undefined) {
      try {
        await api.setReaction(acknowledge, "confused");
      } catch (reactionError) {
        console.error(`Could not mark comment ${acknowledge} rejected: ${reactionError.message}`);
      }
    }
    throw error;
  }
}

async function main() {
  const eventName = process.env.GITHUB_EVENT_NAME;
  const eventPath = process.env.GITHUB_EVENT_PATH;
  const issueNumber = Number(process.env.PHOENIX_ROADMAP_ISSUE_NUMBER);
  const token = process.env.GITHUB_TOKEN;
  const [owner, repo] = String(process.env.GITHUB_REPOSITORY ?? "").split("/");
  if (!eventName || !eventPath || !Number.isSafeInteger(issueNumber) || issueNumber <= 0 || !token || !owner || !repo) {
    throw new Error("GITHUB_EVENT_NAME, GITHUB_EVENT_PATH, GITHUB_REPOSITORY, GITHUB_TOKEN, and PHOENIX_ROADMAP_ISSUE_NUMBER are required");
  }
  const event = JSON.parse(await fs.readFile(eventPath, "utf8"));
  const result = await run({
    eventName,
    event,
    configuredIssueNumber: issueNumber,
    coordinatorHarness: process.env.PHOENIX_ROADMAP_COORDINATOR_HARNESS || undefined,
    api: githubApi({ owner, repo, issueNumber, token }),
  });
  console.log(JSON.stringify(result));
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
