// Phoenix Chains v1 — sidebar grouping helpers (REQ-CHN-002).
//
// Pure, side-effect-free functions that derive chain membership and the
// sidebar's display order from a flat conversation list. These are kept
// independent of React so they can be memoized at the call site and unit
// tested without DOM machinery.
//
// Membership is derived (not stored) by walking `continued_in_conv_id`
// pointers across the loaded conversation list. A "chain" is two or more
// linked conversations; lone conversations with neither a successor nor a
// predecessor are NOT treated as chains (REQ-CHN-002 — minimum 2 members).

import type { Conversation } from '../api';

/**
 * Compute the chain root of every conversation in the list.
 *
 * For each conversation, walks back along its predecessor pointer (the
 * conversation whose `continued_in_conv_id` points at it) until reaching a
 * conversation with no predecessor — that's the root. If the chain has only
 * one member (no successor and no predecessor), the conversation maps to
 * `null`.
 *
 * The walk is bounded: any cycle (which the backend's single-successor
 * invariant should prevent, but a defense in depth is cheap) breaks out
 * after seeing a previously-visited node, treating the loop entry as the
 * root.
 *
 * Return: Map<conv_id, root_conv_id | null>. `null` means "standalone /
 * not part of any chain."
 */
export function computeChainRoots(
  conversations: readonly Conversation[],
): Map<string, string | null> {
  // Build an id -> conversation map and a successor -> predecessor map so
  // we can walk backward as well as forward without scanning the list per
  // step.
  const byId = new Map<string, Conversation>();
  const predecessorOf = new Map<string, string>();
  for (const c of conversations) {
    byId.set(c.id, c);
  }
  for (const c of conversations) {
    if (c.continued_in_conv_id && byId.has(c.continued_in_conv_id)) {
      predecessorOf.set(c.continued_in_conv_id, c.id);
    }
  }

  // Memoized walk: convId -> root (or null when standalone).
  const rootCache = new Map<string, string | null>();

  const findRoot = (id: string): string | null => {
    const cached = rootCache.get(id);
    if (cached !== undefined) return cached;

    // Walk to the head of the chain (no predecessor).
    const visited = new Set<string>();
    let cursor = id;
    let head = id;
    for (;;) {
      if (visited.has(cursor)) {
        // Cycle defense; treat the entry point as the root.
        head = cursor;
        break;
      }
      visited.add(cursor);
      const prev = predecessorOf.get(cursor);
      if (!prev) {
        head = cursor;
        break;
      }
      cursor = prev;
    }

    // Standalone iff the head has no successor (its own
    // `continued_in_conv_id` resolves to nothing in this list).
    const headConv = byId.get(head);
    const hasSuccessor =
      !!headConv?.continued_in_conv_id && byId.has(headConv.continued_in_conv_id);
    const root = hasSuccessor ? head : null;

    // Cache the result for every node on the walked path so subsequent
    // queries are O(1).
    for (const v of visited) {
      rootCache.set(v, root);
    }
    return root;
  };

  const out = new Map<string, string | null>();
  for (const c of conversations) {
    out.set(c.id, findRoot(c.id));
  }
  return out;
}

/**
 * A single ordered entry rendered in the sidebar — either a standalone
 * conversation or a chain block of ≥ 2 members.
 */
export type SidebarItem =
  | { kind: 'single'; conversation: Conversation }
  | {
      kind: 'chain';
      rootId: string;
      /** Display name: chain_name on root, falling back to root's slug. */
      displayName: string;
      /** Members in chain order (root → leaf). */
      members: Conversation[];
      /** ID of the latest member (max updated_at) — visually emphasized. */
      latestMemberId: string;
    };

/**
 * Group a recency-sorted (`updated_at DESC`) conversation list into a
 * display sequence interleaving standalone rows and chain blocks.
 *
 * Algorithm (matches `specs/chains/design.md` "Sidebar Grouping"):
 *   1. Each chain block is positioned at the recency rank of its
 *      most-recent member.
 *   2. Within a block, members are listed in chain order (root → latest)
 *      independent of their own `updated_at`.
 *   3. Standalone conversations remain interleaved by recency between
 *      blocks.
 *
 * The input list is assumed to be sorted by `updated_at` DESC. Output
 * order preserves the relative recency rank of every chain (its
 * most-recent member) and every standalone.
 */
export function groupConversationsForSidebar(
  conversations: readonly Conversation[],
  chainRoots: Map<string, string | null>,
): SidebarItem[] {
  // Bucket members by root.
  const membersByRoot = new Map<string, Conversation[]>();
  for (const c of conversations) {
    const root = chainRoots.get(c.id);
    if (root) {
      const list = membersByRoot.get(root) ?? [];
      list.push(c);
      membersByRoot.set(root, list);
    }
  }

  // Order each chain's members in chain order (root → leaf) by walking
  // continued_in_conv_id forward from the root. Members that don't appear
  // in the list (deleted / missing) are skipped silently — the chain
  // remains coherent through whatever members are available.
  const orderedMembers = new Map<string, Conversation[]>();
  for (const [rootId, ms] of membersByRoot) {
    const byId = new Map<string, Conversation>();
    for (const m of ms) byId.set(m.id, m);
    const ordered: Conversation[] = [];
    let cursor: string | null | undefined = rootId;
    const seen = new Set<string>();
    while (cursor && byId.has(cursor) && !seen.has(cursor)) {
      const m: Conversation = byId.get(cursor)!;
      ordered.push(m);
      seen.add(cursor);
      cursor = m.continued_in_conv_id ?? null;
    }
    // Append any orphaned members (chain pointer drift) at the end so we
    // never silently drop them — defense-in-depth against backend bugs.
    for (const m of ms) {
      if (!seen.has(m.id)) ordered.push(m);
    }
    orderedMembers.set(rootId, ordered);
  }

  // Walk the recency-sorted list and emit one item per chain (at its
  // most-recent member's position) and one item per standalone.
  const out: SidebarItem[] = [];
  const emittedChain = new Set<string>();
  for (const c of conversations) {
    const root = chainRoots.get(c.id);
    if (root) {
      if (emittedChain.has(root)) continue;
      emittedChain.add(root);
      const members = orderedMembers.get(root) ?? [];
      if (members.length === 0) continue;
      const rootConv = members[0]!;
      const displayName = rootConv.chain_name ?? rootConv.slug;
      // Latest member = the non-root member with the largest updated_at,
      // matching the backend `ChainView` rule (see src/api/chains.rs). The
      // root is excluded because edits to root metadata (e.g. setting
      // chain_name) bump its updated_at without representing real work; if
      // the root were eligible it would incorrectly become "latest" after
      // a rename. Members are ordered root-first by chain order, so
      // members[0] is always the root and members[1..] are non-root.
      // A chain has length ≥ 2 (REQ-CHN-002), so members[1] always exists.
      let latest = members[1]!;
      for (let i = 2; i < members.length; i++) {
        const m = members[i]!;
        if (m.updated_at > latest.updated_at) latest = m;
      }
      out.push({
        kind: 'chain',
        rootId: root,
        displayName,
        members,
        latestMemberId: latest.id,
      });
    } else {
      out.push({ kind: 'single', conversation: c });
    }
  }
  return out;
}
