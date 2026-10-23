import { useCallback, useEffect, useRef, useState } from 'react';
import { parse } from 'valibot';
import { SseInitDataSchema } from '../../sseSchemas';
import { ProductConversationFixture } from '../productConversation';
import { ConsumptionObserver, type ConsumptionController } from './ConsumptionObserver';
import type { ConsumptionReceipt } from './observeConsumption';
import { makeTouchScenario, streamMarker } from './scenario';
import { assertTouchStreamOwner } from './streamOwner';
import { delegateTouchRefresh } from './refreshRoute';
import { alignTouchInitialWatermark } from './initialWatermark';
import { continuationTarget } from './continuationTarget';

type Listener = EventListenerOrEventListenerObject;
type Wire = { type: 'token'; text: string } | { type: 'message_updated'; messageId: string; text: string };
export interface TouchFixtureBridge {
  diagnostic(): { initialized: boolean; sourceState: number | null; tokenListeners: number;
    emitted: number; observer: ReturnType<ConsumptionController['diagnostic']> | null };
  ready(): boolean;
  arm(phase: string, tokens: readonly string[]): void;
  append(phase: string, tokens: readonly string[]): void;
  disarm(): void;
  emit(wire: Wire): void;
  status(): { initialized: boolean; emitted: number; tokenText: string; transcriptId: string; consumption: ConsumptionReceipt };
}
declare global { interface Window { __touchFixture?: TouchFixtureBridge } }

export function TouchReadingFixture() {
  const [scenario] = useState(makeTouchScenario);
  const liveId = scenario.snapshot.latest_transcript_row_id;
  const controller = useRef<ConsumptionController | null>(null);
  const receipt = useRef<ConsumptionReceipt>({ status: 'UNOBSERVED', phase: null, reason: 'not attached' });
  const publish = useCallback((next: ConsumptionReceipt) => { receipt.current = next; }, []);
  const attach = useCallback((next: ConsumptionController | null) => {
    controller.current = next;
    receipt.current = { status: 'UNOBSERVED', phase: null, reason: next ? 'attached; not armed' : 'detached' };
  }, []);

  useEffect(() => {
    if (!liveId || scenario.snapshot.writable_transcript_row_id !== liveId) {
      throw new Error('Touch fixture has no authoritative writable live identity');
    }
    const Base = globalThis.EventSource;
    if (Number(document.documentElement.dataset['productConversationFixtureEventSourceOpens'] ?? 0) !== 0) {
      throw new Error('Touch adapter missed pre-body EventSource installation');
    }
    const runId = crypto.randomUUID();
    let requestOrdinal = 0;
    let alive = true;
    type Source = { source: EventSource; listeners: Map<string, Set<Listener>>;
      init: { incarnation: string; sequence: number } | null };
    let active: Source | null = null;
    let sequence = 0;
    let emitted = 0;
    let tokenText = '';
    let plan: { owner: Source; controller: ConsumptionController; tokens: readonly string[];
      index: number; requestId: string } | null = null;
    const invalidate = () => {
      plan = null;
      controller.current?.disarm();
      receipt.current = { status: 'UNOBSERVED', phase: null, reason: 'source/phase invalidated' };
    };
    const invoke = (listener: Listener, source: EventSource, event: Event) => {
      if (typeof listener === 'function') listener.call(source, event);
      else listener.handleEvent(event);
    };
    const Adapter = new Proxy(Base, {
      construct(target, args) {
        const refresh = delegateTouchRefresh(target, args, window.location.href);
        if (refresh) return refresh;
        assertTouchStreamOwner(String(args[0]), window.location.href, liveId);
        const source = Reflect.construct(target, args) as EventSource;
        invalidate();
        const entry: Source = { source, listeners: new Map(), init: null };
        active = entry;
        const add = source.addEventListener.bind(source);
        const remove = source.removeEventListener.bind(source);
        const initWrappers = new Map<Listener, EventListener>();
        source.addEventListener = ((type: string, listener: Listener, options?: boolean | AddEventListenerOptions) => {
          const set = entry.listeners.get(type) ?? new Set<Listener>();
          set.add(listener);
          entry.listeners.set(type, set);
          if (type !== 'init') { add(type, listener, options); return; }
          const wrapped: EventListener = event => {
            if (!alive || active !== entry) return;
            invalidate();
            entry.init = null;
            const parsed = alignTouchInitialWatermark(parse(SseInitDataSchema, JSON.parse((event as MessageEvent<string>).data)));
            if (parsed.conversation.id !== liveId || !parsed.stream_incarnation
              || !Number.isSafeInteger(parsed.last_sequence_id) || parsed.last_sequence_id < 0) {
              throw new Error('Unexpected touch init identity/watermark');
            }
            // Production handler validates/reduces before readiness is exposed.
            invoke(listener, source, new MessageEvent('init', { data: JSON.stringify(parsed) }));
            sequence = parsed.last_sequence_id;
            tokenText = '';
            entry.init = { incarnation: parsed.stream_incarnation, sequence };
          };
          initWrappers.set(listener, wrapped);
          add(type, wrapped, options);
        }) as EventSource['addEventListener'];
        source.removeEventListener = ((type: string, listener: Listener, options?: boolean | EventListenerOptions) => {
          entry.listeners.get(type)?.delete(listener);
          remove(type, type === 'init' ? initWrappers.get(listener) ?? listener : listener, options);
          if (type === 'init') initWrappers.delete(listener);
        }) as EventSource['removeEventListener'];
        return source;
      },
    });
    globalThis.EventSource = Adapter;
    const bridge: TouchFixtureBridge = {
      ready: () => alive && Boolean(active?.init) && active?.source.readyState === 1
        && Boolean(active.listeners.get('token')?.size) && controller.current !== null,
      status: () => ({ initialized: Boolean(active?.init), emitted, tokenText,
        transcriptId: liveId, consumption: receipt.current }),
      diagnostic: () => ({ initialized: Boolean(active?.init), sourceState: active?.source.readyState ?? null,
        tokenListeners: active?.listeners.get('token')?.size ?? 0, emitted,
        observer: controller.current?.diagnostic() ?? null }),
      disarm: invalidate,
      arm(phase, tokens) {
        invalidate();
        const owner = active;
        const observer = controller.current;
        if (!bridge.ready() || !owner?.init || !observer || !phase || tokens.length === 0
          || tokens.some(text => typeof text !== 'string' || text.length === 0)) {
          throw new Error('Cannot arm touch phase without live identity and explicit token plan');
        }
        const finalSequence = sequence + tokens.length;
        if (!Number.isSafeInteger(finalSequence)) throw new Error('Unsafe planned sequence');
        const immutableTokens = Object.freeze([...tokens]);
        const requestId = `${runId}:${++requestOrdinal}`;
        plan = { owner, controller: observer, tokens: immutableTokens, index: 0, requestId };
        tokenText = '';
        observer.arm({ phase, requestId, streamIncarnation: owner.init.incarnation,
          eventSequence: finalSequence, tokenSequence: finalSequence, text: immutableTokens.join('') });
      },
      append(phase, tokens) {
        const owner = active, observer = controller.current, previous = plan;
        try {
          if (!bridge.ready() || !owner?.init || !observer || !previous || previous.owner !== owner
            || previous.controller !== observer || previous.index !== previous.tokens.length) {
            throw new Error('No completed owned request to continue');
          }
          const immutableTokens = Object.freeze([...tokens]);
          const target = continuationTarget({ receipt: receipt.current, phase, tokens: immutableTokens, liveId,
            requestId: previous.requestId, streamIncarnation: owner.init.incarnation, sequence, text: tokenText });
          invalidate();
          plan = { owner, controller: observer, tokens: immutableTokens, index: 0, requestId: previous.requestId };
          observer.arm(target);
        } catch (error) { invalidate(); throw error; }
      },
      emit(wire) {
        if (!bridge.ready() || !active) throw new Error('Touch EventSource not live/initialized');
        const callbacks = active.listeners.get(wire.type);
        if (!callbacks?.size) throw new Error(`No production listener for ${wire.type}`);
        if (wire.type === 'token' && (!plan || plan.owner !== active || plan.controller !== controller.current
          || plan.tokens[plan.index] !== wire.text)) {
          invalidate();
          throw new Error('Token does not match armed immutable workload');
        }
        if (wire.type === 'message_updated') invalidate();
        const data = wire.type === 'token'
          ? { sequence_id: ++sequence, request_id: plan!.requestId, text: wire.text }
          : { sequence_id: ++sequence, opening: null, message_id: wire.messageId,
            transcript_generation: 1, content: [{ type: 'text', text: wire.text }], display_data: null };
        const event = new MessageEvent(wire.type, { data: JSON.stringify(data) });
        for (const listener of callbacks) invoke(listener, active.source, event);
        emitted++;
        if (wire.type === 'token') { tokenText += wire.text; plan!.index++; }
      },
    };
    window.__touchFixture = bridge;
    document.documentElement.dataset['touchReadingSource'] = streamMarker;
    return () => {
      alive = false;
      invalidate();
      active = null;
      if (window.__touchFixture === bridge) delete window.__touchFixture;
      delete document.documentElement.dataset['touchReadingSource'];
      if (globalThis.EventSource === Adapter) globalThis.EventSource = Base;
    };
  }, [liveId, scenario]);
  return <ProductConversationFixture scenario={scenario}
    observer={<ConsumptionObserver liveId={liveId} publish={publish} attach={attach} />} />;
}
