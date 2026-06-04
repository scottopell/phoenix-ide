// Chapter derivation for the conversation navigation strip.
//
// Pure transform: HistoricalUnit[] -> Chapter[]. A sibling to
// `buildRenderUnits` (renderUnits.ts) — it does NOT change which messages
// render or how they group; it only classifies which already-built render
// units are skimmable "chapters" the nav strip can jump to.
//
// Chapters are the conversation's signposts:
//   - every user prompt (kind: 'prompt')
//   - every assistant text block at/over the significance threshold
//     (kind: 'prose'), reusing `isSignificantText` so the nav strip and
//     the compact-density feature agree on what counts as "significant".
//
// The emitted `unitIndex` is the position of the chapter's unit within the
// SAME `historicalUnits` array the caller passed in — which is the array
// MessageList feeds to virtuoso (followed by tail units). The nav strip
// uses it directly as the virtuoso `scrollToIndex` target.

import type { HistoricalUnit } from './renderUnits';
import { isSignificantText } from '../hooks/useDensity';

export type ChapterKind = 'prompt' | 'prose';

export interface Chapter {
  /** Index of this chapter's unit within the `historicalUnits` array — i.e.
   *  its virtuoso item index (tail units always follow historical units, so
   *  a historical unit's index is identical in both coordinate spaces). */
  unitIndex: number;
  kind: ChapterKind;
  /** Truncated prompt / first line of prose, for the pill label. */
  label: string;
  /** Sequence id of the underlying message, when present — used for
   *  scroll-spy matching against rendered `data-sequence-id` nodes. Pending
   *  user messages and skill units have no sequence id yet. */
  sequenceId: number | undefined;
}

const LABEL_MAX_CHARS = 40;

/** Collapse whitespace and clip to a single readable line for a pill. */
export function truncateLabel(text: string, max = LABEL_MAX_CHARS): string {
  const oneLine = text.replace(/\s+/g, ' ').trim();
  if (oneLine.length <= max) return oneLine;
  return `${oneLine.slice(0, max - 1).trimEnd()}…`;
}

function userText(unit: Extract<HistoricalUnit, { kind: 'user' | 'pending_user' }>): string {
  // `pending_user` carries a QueuedMessage (text on the object); `user`
  // carries a persisted Message (text under content). Both surface the same
  // prompt string.
  if (unit.kind === 'pending_user') {
    return typeof unit.message.text === 'string' ? unit.message.text : '';
  }
  const content = unit.message.content as { text?: string } | undefined;
  return typeof content?.text === 'string' ? content.text : '';
}

/** First significant assistant text block in an agent turn, if any. Returns
 *  the block text (untruncated) when it meets the significance threshold,
 *  else undefined. Mirrors how `AgentMessage` reads `ContentBlock[]`. */
function firstSignificantProse(
  unit: Extract<HistoricalUnit, { kind: 'agent_turn' }>,
): string | undefined {
  const blocks = unit.agent.content;
  if (!Array.isArray(blocks)) return undefined;
  for (const block of blocks) {
    if (block.type === 'text' && typeof block.text === 'string' && isSignificantText(block.text)) {
      return block.text;
    }
  }
  return undefined;
}

/**
 * Derive the conversation chapters from the already-built historical render
 * units. The `unitIndex` of each chapter is its position in `historicalUnits`,
 * which equals its virtuoso item index.
 */
export function buildConversationChapters(historicalUnits: HistoricalUnit[]): Chapter[] {
  const chapters: Chapter[] = [];

  historicalUnits.forEach((unit, unitIndex) => {
    switch (unit.kind) {
      case 'user':
      case 'pending_user': {
        const text = userText(unit);
        if (text.trim().length === 0) break;
        chapters.push({
          unitIndex,
          kind: 'prompt',
          label: truncateLabel(text),
          sequenceId: unit.kind === 'user' ? unit.message.sequence_id : undefined,
        });
        break;
      }
      case 'agent_turn': {
        const prose = firstSignificantProse(unit);
        if (prose === undefined) break;
        chapters.push({
          unitIndex,
          kind: 'prose',
          label: truncateLabel(prose),
          sequenceId: unit.agent.sequence_id,
        });
        break;
      }
      // 'skill' and 'system' units are not chapters: skills are command
      // invocations (not prose to navigate to) and system messages are
      // out-of-band notices. They still occupy a slot in historicalUnits,
      // so chapters past them keep correct unitIndex values via forEach.
      default:
        break;
    }
  });

  return chapters;
}
