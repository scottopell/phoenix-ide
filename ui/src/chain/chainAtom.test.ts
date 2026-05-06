import { describe, it, expect } from 'vitest';
import {
  chainReducer,
  createInitialChainAtom,
  type ChainAtom,
  type ChainAction,
} from './chainAtom';
import type { ChainView } from '../api';

function makeView(overrides: Partial<ChainView> = {}): ChainView {
  return {
    root_conv_id: 'root-1',
    chain_name: null,
    display_name: 'My chain',
    archived: false,
    members: [],
    qa_history: [],
    current_member_count: 1,
    current_total_messages: 0,
    snapshot_member_count: 1,
    snapshot_total_messages: 0,
    ...overrides,
  } as ChainView;
}

function dispatch(atom: ChainAtom, action: ChainAction): ChainAtom {
  return chainReducer(atom, action);
}

describe('chainReducer', () => {
  describe('createInitialChainAtom', () => {
    it('starts in loading state with no chain', () => {
      const atom = createInitialChainAtom();
      expect(atom.chain).toBeNull();
      expect(atom.loading).toBe(true);
      expect(atom.loadError).toBeNull();
      expect(atom.inflight).toEqual({});
      expect(atom.inflightOrder).toEqual([]);
      expect(atom.draft).toBe('');
      expect(atom.submitting).toBe(false);
      expect(atom.sseLost).toBe(false);
    });
  });

  describe('LOAD_BEGIN / LOAD_OK / LOAD_FAIL', () => {
    it('LOAD_OK lifts loading and clears any error', () => {
      const atom: ChainAtom = {
        ...createInitialChainAtom(),
        loadError: 'transient',
      };
      const view = makeView();
      const next = dispatch(atom, { type: 'LOAD_OK', view });
      expect(next.chain).toBe(view);
      expect(next.loading).toBe(false);
      expect(next.loadError).toBeNull();
    });

    it('LOAD_FAIL preserves an existing chain snapshot (REQ-CHN-005)', () => {
      const view = makeView();
      const atom: ChainAtom = {
        ...createInitialChainAtom(),
        chain: view,
        loading: false,
      };
      const next = dispatch(atom, { type: 'LOAD_FAIL', error: 'network' });
      expect(next.chain).toBe(view);
      expect(next.loadError).toBe('network');
      expect(next.loading).toBe(false);
    });

    it('LOAD_BEGIN is a no-op when already loading and no error', () => {
      const atom = createInitialChainAtom();
      const next = dispatch(atom, { type: 'LOAD_BEGIN' });
      expect(next).toBe(atom);
    });

    it('LOAD_BEGIN clears a stale error and re-asserts loading', () => {
      const atom: ChainAtom = {
        ...createInitialChainAtom(),
        loading: false,
        loadError: 'old',
      };
      const next = dispatch(atom, { type: 'LOAD_BEGIN' });
      expect(next.loading).toBe(true);
      expect(next.loadError).toBeNull();
    });
  });

  describe('OPTIMISTIC_INFLIGHT_ADD', () => {
    it('adds a new in-flight entry, appends to order, clears draft', () => {
      const atom: ChainAtom = {
        ...createInitialChainAtom(),
        draft: 'what is the meaning of life?',
      };
      const next = dispatch(atom, {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'qa-1',
        question: 'what is the meaning of life?',
      });
      expect(next.inflight['qa-1']).toEqual({
        chainQaId: 'qa-1',
        question: 'what is the meaning of life?',
        answer: '',
        preToken: true,
        error: null,
      });
      expect(next.inflightOrder).toEqual(['qa-1']);
      expect(next.draft).toBe('');
    });

    it('is idempotent on repeated dispatch (defense against retry double-fire)', () => {
      let atom = createInitialChainAtom();
      atom = dispatch(atom, {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'qa-1',
        question: 'q',
      });
      const before = atom;
      atom = dispatch(atom, {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'qa-1',
        question: 'q',
      });
      expect(atom).toBe(before);
    });
  });

  describe('TOKEN_APPENDED', () => {
    it('appends delta to the matching in-flight entry and flips preToken', () => {
      let atom = dispatch(createInitialChainAtom(), {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'qa-1',
        question: 'q',
      });
      atom = dispatch(atom, { type: 'TOKEN_APPENDED', chainQaId: 'qa-1', delta: 'Hel' });
      atom = dispatch(atom, { type: 'TOKEN_APPENDED', chainQaId: 'qa-1', delta: 'lo' });
      expect(atom.inflight['qa-1']?.answer).toBe('Hello');
      expect(atom.inflight['qa-1']?.preToken).toBe(false);
    });

    it('drops tokens for unknown chain_qa_id (sibling-tab Q&A)', () => {
      const atom = createInitialChainAtom();
      const next = dispatch(atom, {
        type: 'TOKEN_APPENDED',
        chainQaId: 'sibling-tab',
        delta: 'something',
      });
      expect(next).toBe(atom);
    });
  });

  describe('INFLIGHT_FAIL / INFLIGHT_DROP', () => {
    it('INFLIGHT_FAIL surfaces the error and replaces partial answer', () => {
      let atom = dispatch(createInitialChainAtom(), {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'qa-1',
        question: 'q',
      });
      atom = dispatch(atom, {
        type: 'INFLIGHT_FAIL',
        chainQaId: 'qa-1',
        error: 'rate limited',
        partialAnswer: 'partial response',
      });
      expect(atom.inflight['qa-1']?.error).toBe('rate limited');
      expect(atom.inflight['qa-1']?.answer).toBe('partial response');
      expect(atom.inflight['qa-1']?.preToken).toBe(false);
    });

    it('INFLIGHT_DROP removes the entry and its order slot', () => {
      let atom = dispatch(createInitialChainAtom(), {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'qa-1',
        question: 'q',
      });
      atom = dispatch(atom, {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'qa-2',
        question: 'r',
      });
      atom = dispatch(atom, { type: 'INFLIGHT_DROP', chainQaId: 'qa-1' });
      expect(atom.inflight['qa-1']).toBeUndefined();
      expect(atom.inflightOrder).toEqual(['qa-2']);
    });
  });

  describe('DRAFT_CHANGED / DRAFT_SET', () => {
    it('DRAFT_CHANGED returns the same atom when value is unchanged', () => {
      const atom = createInitialChainAtom();
      const next = dispatch(atom, { type: 'DRAFT_CHANGED', value: '' });
      expect(next).toBe(atom);
    });

    it('DRAFT_CHANGED updates only the draft field', () => {
      const atom = createInitialChainAtom();
      const next = dispatch(atom, { type: 'DRAFT_CHANGED', value: 'hello' });
      expect(next.draft).toBe('hello');
    });

    it('DRAFT_SET behaves the same as DRAFT_CHANGED today (re-ask path)', () => {
      const atom = createInitialChainAtom();
      const next = dispatch(atom, { type: 'DRAFT_SET', value: 'replay this' });
      expect(next.draft).toBe('replay this');
    });
  });

  describe('SUBMIT lifecycle', () => {
    it('SUBMIT_BEGIN sets submitting; SUBMIT_OK clears it', () => {
      let atom = dispatch(createInitialChainAtom(), { type: 'SUBMIT_BEGIN' });
      expect(atom.submitting).toBe(true);
      atom = dispatch(atom, { type: 'SUBMIT_OK' });
      expect(atom.submitting).toBe(false);
    });

    it('SUBMIT_FAIL stores the error and keeps draft for retry', () => {
      let atom: ChainAtom = {
        ...createInitialChainAtom(),
        draft: 'unsent',
        submitting: true,
      };
      atom = dispatch(atom, { type: 'SUBMIT_FAIL', error: '503' });
      expect(atom.submitting).toBe(false);
      expect(atom.draft).toBe('unsent');
      expect(atom.loadError).toBe('503');
    });
  });

  describe('SSE_LOST / SSE_RESTORED', () => {
    it('SSE_LOST is idempotent', () => {
      const lost = dispatch(createInitialChainAtom(), { type: 'SSE_LOST' });
      expect(lost.sseLost).toBe(true);
      const same = dispatch(lost, { type: 'SSE_LOST' });
      expect(same).toBe(lost);
    });

    it('SSE_RESTORED clears the flag', () => {
      const lost = dispatch(createInitialChainAtom(), { type: 'SSE_LOST' });
      const restored = dispatch(lost, { type: 'SSE_RESTORED' });
      expect(restored.sseLost).toBe(false);
    });
  });

  describe('INFLIGHT_RECONCILE_ID', () => {
    it('rekeys the entry from temp id to real id, preserving order slot', () => {
      let atom = dispatch(createInitialChainAtom(), {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'temp-1',
        question: 'q',
      });
      atom = dispatch(atom, { type: 'TOKEN_APPENDED', chainQaId: 'temp-1', delta: 'partial' });
      atom = dispatch(atom, {
        type: 'INFLIGHT_RECONCILE_ID',
        tempId: 'temp-1',
        realId: 'qa-real-7',
      });
      expect(atom.inflight['temp-1']).toBeUndefined();
      expect(atom.inflight['qa-real-7']).toBeDefined();
      // The accumulated answer carries over.
      expect(atom.inflight['qa-real-7']?.answer).toBe('partial');
      // The chainQaId field on the entry now points at the real id.
      expect(atom.inflight['qa-real-7']?.chainQaId).toBe('qa-real-7');
      // Order slot rekeyed in place.
      expect(atom.inflightOrder).toEqual(['qa-real-7']);
    });

    it('is a no-op when temp id is unknown', () => {
      const atom = createInitialChainAtom();
      const next = dispatch(atom, {
        type: 'INFLIGHT_RECONCILE_ID',
        tempId: 'never-existed',
        realId: 'whatever',
      });
      expect(next).toBe(atom);
    });

    it('is a no-op when temp id equals real id', () => {
      let atom = dispatch(createInitialChainAtom(), {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'same',
        question: 'q',
      });
      const before = atom;
      atom = dispatch(atom, {
        type: 'INFLIGHT_RECONCILE_ID',
        tempId: 'same',
        realId: 'same',
      });
      expect(atom).toBe(before);
    });

    it('drops temp entry if real id already occupied (sibling-tab race)', () => {
      let atom = dispatch(createInitialChainAtom(), {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'temp-1',
        question: 'mine',
      });
      atom = dispatch(atom, {
        type: 'OPTIMISTIC_INFLIGHT_ADD',
        chainQaId: 'qa-real-7',
        question: 'sibling',
      });
      atom = dispatch(atom, {
        type: 'INFLIGHT_RECONCILE_ID',
        tempId: 'temp-1',
        realId: 'qa-real-7',
      });
      expect(atom.inflight['temp-1']).toBeUndefined();
      expect(atom.inflight['qa-real-7']?.question).toBe('sibling');
      expect(atom.inflightOrder).toEqual(['qa-real-7']);
    });
  });
});
