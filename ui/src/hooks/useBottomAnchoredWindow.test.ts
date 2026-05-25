// Tests for the pure helpers used by useBottomAnchoredWindow.
//
// The hook itself integrates these with React + IntersectionObserver +
// DOM; that integration is exercised by MessageList.test.tsx. This file
// covers the structural decisions: initial-window placement under
// saved-anchor variants, and spacer-height summation across the
// per-kind estimates.

import { describe, it, expect } from 'vitest';
import {
  computeInitialStart,
  computeSpacerHeight,
  INITIAL_WINDOW,
  RESTORE_OVERSCAN,
  KIND_ESTIMATES,
  type SavedScrollAnchor,
} from './useBottomAnchoredWindow';
import type { Message } from '../api';
import type { HistoricalUnit } from '../conversation/renderUnits';

function mkUser(key: string): HistoricalUnit {
  const m = {
    message_id: key,
    sequence_id: 0,
    conversation_id: 'c',
    message_type: 'user',
    content: { text: 'x' },
    created_at: '',
  } as Message;
  return { kind: 'user', key, message: m };
}

function mkAgentTurn(key: string): HistoricalUnit {
  const m = {
    message_id: key,
    sequence_id: 0,
    conversation_id: 'c',
    message_type: 'agent',
    content: [],
    created_at: '',
  } as Message;
  return {
    kind: 'agent_turn',
    key,
    agent: m,
    toolResultsByUseId: new Map(),
    isFirstInTurn: true,
  };
}

function mkSystem(key: string): HistoricalUnit {
  const m = {
    message_id: key,
    sequence_id: 0,
    conversation_id: 'c',
    message_type: 'system',
    content: { text: 'note' },
    created_at: '',
  } as Message;
  return { kind: 'system', key, message: m };
}

function mkSkill(key: string): HistoricalUnit {
  const m = {
    message_id: key,
    sequence_id: 0,
    conversation_id: 'c',
    message_type: 'skill',
    content: { text: '' } as Message['content'],
    created_at: '',
  } as Message;
  return { kind: 'skill', key, message: m };
}

describe('computeInitialStart', () => {
  it('returns 0 when there are fewer units than INITIAL_WINDOW', () => {
    const units = Array.from({ length: 5 }, (_, i) => mkUser(`u${i}`));
    expect(computeInitialStart(units, null)).toBe(0);
  });

  it('bottom-pins to the last INITIAL_WINDOW units when no anchor is provided', () => {
    const units = Array.from({ length: 50 }, (_, i) => mkUser(`u${i}`));
    expect(computeInitialStart(units, null)).toBe(50 - INITIAL_WINDOW);
  });

  it('returns 0 when units array is empty (regardless of anchor)', () => {
    expect(computeInitialStart([], null)).toBe(0);
    expect(
      computeInitialStart([], { topVisibleUnitKey: 'missing', offsetWithinUnit: 0 }),
    ).toBe(0);
  });

  it('widens the window so the anchored unit + RESTORE_OVERSCAN above is rendered', () => {
    const units = Array.from({ length: 100 }, (_, i) => mkUser(`u${i}`));
    const anchor: SavedScrollAnchor = {
      topVisibleUnitKey: 'u50',
      offsetWithinUnit: 12,
    };
    expect(computeInitialStart(units, anchor)).toBe(50 - RESTORE_OVERSCAN);
  });

  it('clamps the anchor start to 0 when the anchored unit is within RESTORE_OVERSCAN of the top', () => {
    const units = Array.from({ length: 100 }, (_, i) => mkUser(`u${i}`));
    const anchor: SavedScrollAnchor = {
      topVisibleUnitKey: 'u2',
      offsetWithinUnit: 0,
    };
    expect(computeInitialStart(units, anchor)).toBe(0);
  });

  it('falls back to bottom-pin when the anchor key is not present in units', () => {
    const units = Array.from({ length: 50 }, (_, i) => mkUser(`u${i}`));
    const anchor: SavedScrollAnchor = {
      topVisibleUnitKey: 'gone',
      offsetWithinUnit: 100,
    };
    expect(computeInitialStart(units, anchor)).toBe(50 - INITIAL_WINDOW);
  });

  it('anchor that points to a unit near the bottom still uses the anchor, not bottom-pin', () => {
    // Bottom-pin would be 100 - 12 = 88. Anchor at u92 - 4 = 88. Both
    // produce the same result here, but the path is anchor-driven —
    // verify by anchoring slightly higher.
    const units = Array.from({ length: 100 }, (_, i) => mkUser(`u${i}`));
    const anchor: SavedScrollAnchor = {
      topVisibleUnitKey: 'u80',
      offsetWithinUnit: 0,
    };
    expect(computeInitialStart(units, anchor)).toBe(80 - RESTORE_OVERSCAN);
  });
});

describe('computeSpacerHeight', () => {
  it('returns 0 when no units are collapsed', () => {
    const units = Array.from({ length: 10 }, (_, i) => mkUser(`u${i}`));
    expect(computeSpacerHeight(units, 0)).toBe(0);
  });

  it('sums per-kind estimates across the collapsed prefix', () => {
    const units: HistoricalUnit[] = [
      mkUser('u1'),
      mkAgentTurn('a1'),
      mkSkill('s1'),
      mkSystem('sys1'),
    ];
    // Collapse all 4 → sum of all four kind estimates.
    const expected =
      KIND_ESTIMATES.user
      + KIND_ESTIMATES.agent_turn
      + KIND_ESTIMATES.skill
      + KIND_ESTIMATES.system;
    expect(computeSpacerHeight(units, 4)).toBe(expected);
  });

  it('only counts the collapsed prefix, not the rendered slice', () => {
    const units: HistoricalUnit[] = [
      mkUser('u1'),         // counted
      mkAgentTurn('a1'),    // counted
      mkSystem('sys1'),     // not counted (rendered)
      mkUser('u2'),         // not counted (rendered)
    ];
    expect(computeSpacerHeight(units, 2)).toBe(
      KIND_ESTIMATES.user + KIND_ESTIMATES.agent_turn,
    );
  });

  it('clamps firstIdx to units.length without overflowing', () => {
    const units = [mkUser('u1'), mkUser('u2')];
    expect(computeSpacerHeight(units, 99)).toBe(2 * KIND_ESTIMATES.user);
  });

  it('a tool-heavy turn does not get a per-tool spacer allocation', () => {
    // The point of REQ-MLRU-002: tool messages are inside the agent_turn
    // unit's map and don't show up as standalone units. So the collapsed
    // prefix for a tool-heavy turn is one agent_turn unit, not many tool
    // units.
    const units: HistoricalUnit[] = [mkAgentTurn('a1')];
    expect(computeSpacerHeight(units, 1)).toBe(KIND_ESTIMATES.agent_turn);
    // Compare against the (incorrect) old model where every tool would
    // have counted: 1 agent + 20 tools at ~360px each ≈ 7600px. The new
    // model allocates ~400px for that same turn.
  });

  describe('with a getHeight lookup', () => {
    it('uses the measured value when present', () => {
      const units: HistoricalUnit[] = [
        mkUser('u1'),
        mkAgentTurn('a1'),
        mkSystem('sys1'),
      ];
      const measurements = new Map<string, number>([
        ['u1', 42],
        ['a1', 250],
        // sys1 absent — falls back to estimate
      ]);
      const get = (k: string) => measurements.get(k);
      expect(computeSpacerHeight(units, 3, get)).toBe(
        42 + 250 + KIND_ESTIMATES.system,
      );
    });

    it('falls back to the kind estimate for unmeasured units', () => {
      const units: HistoricalUnit[] = [mkUser('u1'), mkAgentTurn('a1')];
      const get = () => undefined;
      expect(computeSpacerHeight(units, 2, get)).toBe(
        KIND_ESTIMATES.user + KIND_ESTIMATES.agent_turn,
      );
    });
  });
});
