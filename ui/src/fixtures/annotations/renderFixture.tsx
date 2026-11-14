import { useContext, useEffect, useMemo } from 'react';
import { InlineReactionContext, type ReactionSource } from '../../conversation/InlineReactionStore';
import { useAnnotationDraft, useDraftActions } from '../../hooks/useDraft';
import { ProductConversationFixture } from '../productConversation/renderFixture';
import { makeVirtualizedReactionScenario } from '../productConversation/scenarios';
import { reactionTextOffset, restoreReactionRange } from '../../components/reactionRange';

export type AnnotationDesignState =
  | '01-empty' | '02-selected' | '03-writing' | '04-discard' | '05-one-annotation'
  | '06-several-annotations' | '07-long-comment' | '08-sending' | '09-failed-send' | '10-sent' | '11-two-annotations' | '12-editing' | '13-after-delete' | '14-many-annotations';

const shortComment = 'Please cover cancellation in this test.';
const longComment = 'Please keep the original result and unfinished draft attached to the same conversation. I want to check cancellation before and after completion, and make sure reconnecting never duplicates the result. This needs a concrete test case rather than a timing assumption.';

function sourceFromParagraph(paragraph: HTMLElement): ReactionSource {
  const owner = paragraph.closest<HTMLElement>('[data-inline-reaction-message]')!;
  const block = paragraph.closest<HTMLElement>('.agent-text-block')!;
  const fragmentId = block.closest<HTMLElement>('[data-fragment-id]')!.dataset['fragmentId']!;
  const range = document.createRange();
  range.selectNodeContents(paragraph);
  return {
    messageId: owner.dataset['inlineReactionMessage']!, sequenceId: 2,
    occurrenceToken: owner.dataset['messageOccurrence'], quote: range.toString(),
    textAnchor: {
      start: { fragmentId, offset: reactionTextOffset(block, range.startContainer, range.startOffset) },
      end: { fragmentId, offset: reactionTextOffset(block, range.endContainer, range.endOffset) },
    },
  };
}

function SeedAnnotationState({ state }: { state: AnnotationDesignState }) {
  const draft = useAnnotationDraft('reaction-work');
  const { setDraft } = useDraftActions('reaction-work');
  const reactions = useContext(InlineReactionContext)!;
  useEffect(() => {
    let frame = 0;
    let cancelled = false;
    let seeded = false;
    let actionStarted = false;
    let navigationComplete = false;
    let nativeSelectionReady = false;
    let selectedSource: ReactionSource | null = null;
    let initialGeometry = '';
    const navigation = new AbortController();
    let previousGeometry = '';
    delete document.documentElement.dataset['annotationDesignReady'];
    const settle = () => {
      if (cancelled) return;
      const transcript = document.getElementById('messages');
      const input = document.querySelector<HTMLTextAreaElement>('#message-input');
      const paragraphs = [...document.querySelectorAll<HTMLElement>('[data-inline-reaction-message="reaction-answer"] .agent-text-block p')];
      if (!transcript || !input || !paragraphs.length) { frame = requestAnimationFrame(settle); return; }
      if (!seeded) {
        const geometry = JSON.stringify([transcript.scrollTop, transcript.scrollHeight, transcript.clientHeight]);
        if (Number(document.documentElement.dataset['productConversationFixtureEventSourceInits'] ?? '0') === 0 || geometry !== initialGeometry) {
          initialGeometry = geometry;
          frame = requestAnimationFrame(settle);
          return;
        }
        seeded = true;
        setDraft('The overall approach looks good. A few notes below.');
        const paragraph = paragraphs[1] ?? paragraphs[0]!;
        const source = sourceFromParagraph(paragraph);
        selectedSource = source;
        if (['02-selected', '03-writing', '04-discard'].includes(state)) {
          reactions.dispatch('pc-inline-reactions', { type: 'select', source, presentation: innerWidth <= 768 ? 'touch-docked' : 'floating' });
          if (state !== '02-selected') reactions.dispatch('pc-inline-reactions', { type: 'edit', body: longComment });
        } else if (state !== '01-empty') {
          draft.add({ source, body: state === '07-long-comment' ? longComment : shortComment });
          if (['06-several-annotations', '11-two-annotations', '12-editing', '13-after-delete', '14-many-annotations'].includes(state)) {
            draft.add({ source: sourceFromParagraph(paragraphs[2] ?? paragraph), body: state === '12-editing' ? longComment : 'Keep this attached to the original conversation.' });
            if (state !== '11-two-annotations') draft.add({ source: sourceFromParagraph(paragraphs.at(-1)!), body: 'Add one browser journey for this guarantee.' });
            if (state === '13-after-delete') draft.remove(draft.getSnapshot()[1]!.id);
            if (state === '14-many-annotations') for (let index = 4; index <= 8; index++) draft.add({ source, body: `Another concrete check, annotation ${index}.` });
          }
          if (state === '08-sending') {
            draft.beginSubmission();
            setDraft('');
          }
        }
        void Promise.resolve(draft.returnToSource(source, navigation.signal)).then(() => {
          if (cancelled) return;
          navigationComplete = true;
          frame = requestAnimationFrame(settle);
        });
        return;
      }
      if (!navigationComplete) { frame = requestAnimationFrame(settle); return; }
      if (state === '02-selected' && !nativeSelectionReady && selectedSource) {
        const range = restoreReactionRange(selectedSource);
        const target = range?.startContainer.parentElement;
        if (!range || !target) { frame = requestAnimationFrame(settle); return; }
        if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
        const pointerType = innerWidth <= 768 ? 'touch' : 'mouse';
        target.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true, pointerType }));
        window.getSelection()!.removeAllRanges();
        window.getSelection()!.addRange(range);
        document.dispatchEvent(new Event('selectionchange'));
        target.dispatchEvent(new PointerEvent('pointerup', { bubbles: true, pointerType }));
        nativeSelectionReady = true;
        frame = requestAnimationFrame(settle);
        return;
      }
      const pill = document.querySelector<HTMLElement>('.reaction-pill');
      if (state === '04-discard' && !actionStarted) {
        const close = pill?.querySelector<HTMLButtonElement>('[aria-label="Dismiss reaction"]');
        if (!close) { frame = requestAnimationFrame(settle); return; }
        actionStarted = true;
        close.click();
        frame = requestAnimationFrame(settle);
        return;
      }
      if (state === '12-editing') {
        if (!actionStarted) {
          const edit = document.querySelector<HTMLButtonElement>('[aria-label="Edit annotation 2"]');
          if (!edit) { frame = requestAnimationFrame(settle); return; }
          actionStarted = true;
          edit.click();
        }
        if (!document.querySelector('.annotation-editor[open]')) { frame = requestAnimationFrame(settle); return; }
      }
      if (state === '09-failed-send' || state === '10-sent') {
        if (!actionStarted) {
          const send = document.querySelector<HTMLButtonElement>('[aria-label="Send message"]');
          if (!send || send.disabled) { frame = requestAnimationFrame(settle); return; }
          actionStarted = true;
          send.click();
        }
        const complete = state === '09-failed-send'
          ? document.querySelector('.input-expansion-error') && draft.getSnapshot().every(item => item.submission === null)
          : document.documentElement.dataset['productConversationFixtureSendCount'] === '1' && draft.getSnapshot().length === 0;
        if (!complete) { frame = requestAnimationFrame(settle); return; }
      }
      if (['02-selected', '03-writing'].includes(state) && !pill) { frame = requestAnimationFrame(settle); return; }
      if (state === '04-discard' && !document.querySelector('.reaction-pill-confirmation')) { frame = requestAnimationFrame(settle); return; }
      if (document.querySelectorAll('.draft-annotation').length !== draft.getSnapshot().length) { frame = requestAnimationFrame(settle); return; }
      const geometry = JSON.stringify([...document.querySelectorAll('#input-area, .reaction-pill, .annotation-editor .selection-dialog__surface')].map(element => element.getBoundingClientRect().toJSON()));
      if (geometry !== previousGeometry || document.fonts.status !== 'loaded') {
        previousGeometry = geometry;
        frame = requestAnimationFrame(settle);
        return;
      }
      document.documentElement.dataset['annotationDesignReady'] = state;
    };
    frame = requestAnimationFrame(settle);
    return () => {
      cancelled = true;
      navigation.abort();
      cancelAnimationFrame(frame);
      delete document.documentElement.dataset['annotationDesignReady'];
    };
  }, [draft, reactions, setDraft, state]);
  return null;
}

export function AnnotationDesignFixture({ state }: { state: AnnotationDesignState }) {
  const scenario = useMemo(() => ({
    ...makeVirtualizedReactionScenario(),
    ...(state === '09-failed-send' ? { sendFailure: 'before-acceptance' as const } : {}),
    ...(state === '08-sending' ? { latestConversationState: { type: 'awaiting_llm' as const } } : {}),
  }), [state]);
  return <ProductConversationFixture scenario={scenario}><SeedAnnotationState state={state} /></ProductConversationFixture>;
}
