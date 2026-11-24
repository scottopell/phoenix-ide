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
