import { useEffect, useState } from 'react';
import { Link, Route, Routes, useParams } from 'react-router-dom';
import * as v from 'valibot';
import type { ProductConversationListRow } from '../api';
import type { CoordinatorObservationTarget } from '../generated/CoordinatorObservationTarget';
import type { ObservationTargetResponse } from '../generated/ObservationTargetResponse';
import { ViewerSlotProvider } from '../contexts/ViewerSlotContext';
import { FileExplorerProvider } from '../components/FileExplorer';
import { ProductConversationPage } from './ProductConversationPage';

const transcriptRowSchema = v.strictObject({
  transcript_row_id: v.string(), slug: v.nullable(v.string()), title: v.nullable(v.string()),
});
const targetConversationFields = {
  product_conversation_id: v.string(), canonical_route: v.string(),
  canonical_root: transcriptRowSchema, latest_transcript_row_id: v.string(), updated_at: v.string(),
};
const ordinaryTargetSchema = v.strictObject({
  ...targetConversationFields,
  lifecycle: v.variant('state', [
    v.strictObject({ state: v.literal('history') }),
    v.strictObject({ state: v.literal('open'), close_action: v.variant('availability', [
      v.strictObject({ availability: v.literal('available') }),
      v.strictObject({ availability: v.literal('unavailable'), reason: v.picklist([
        'active_close_attempt', 'awaiting_task_approval', 'awaiting_continuation', 'handed_off_without_continuation',
      ]) }),
    ]) }),
  ]),
  presentation: v.variant('kind', [
    v.strictObject({ kind: v.literal('needs_action'), display_name: v.string() }),
    v.strictObject({ kind: v.literal('state'), display_name: v.string(), presentation_mode: v.string() }),
  ]),
}) satisfies v.GenericSchema<unknown, ProductConversationListRow>;
const coordinatorTargetSchema = v.strictObject(targetConversationFields) satisfies v.GenericSchema<unknown, CoordinatorObservationTarget>;
const observationTargetSchema = v.variant('kind', [
  v.strictObject({ kind: v.literal('ordinary'), conversation: ordinaryTargetSchema }),
  v.strictObject({ kind: v.literal('coordinator'), conversation: coordinatorTargetSchema }),
]) satisfies v.GenericSchema<unknown, ObservationTargetResponse>;

function PersistedConversation({ global = false }: { global?: boolean }) {
  const { slug } = useParams<{ slug: string }>();
  return <ViewerSlotProvider browserSessionActive={false}><FileExplorerProvider>
    <ProductConversationPage productId={slug} {...(global ? { globalShell: { composerQuickAction: undefined, stateBarExtension: undefined } } : {})} />
  </FileExplorerProvider></ViewerSlotProvider>;
}

function PersistedConversationList() {
  const [target, setTarget] = useState<ObservationTargetResponse | null>(null);
  const [error, setError] = useState(false);
  useEffect(() => {
    const controller = new AbortController();
    fetch('/api/observation/target', { signal: controller.signal, cache: 'no-store' })
      .then(async response => {
        if (!response.ok) throw new Error('Observation target discovery failed');
        return v.parse(observationTargetSchema, await response.json());
      })
      .then(result => { if (!controller.signal.aborted) setTarget(result); })
      .catch(() => { if (!controller.signal.aborted) setError(true); });
    return () => controller.abort();
  }, []);
  if (error) return <div role="alert">Persisted observation target unavailable.</div>;
  if (!target) return <div role="status">Loading persisted observation target…</div>;
  const name = target.kind === 'ordinary' ? target.conversation.presentation.display_name
    : target.conversation.canonical_root.title ?? 'Global coordinator';
  return <main><h1>Persisted conversations</h1>
    <Link to={target.conversation.canonical_route}>{name}</Link>
  </main>;
}

export function PersistedObservation() {
  return <><nav aria-label="Observation"><Link to="/">Persisted conversations</Link> · Read-only observation</nav>
    <Routes>
      <Route path="/" element={<PersistedConversationList />} />
      <Route path="/global" element={<PersistedConversationList />} />
      <Route path="/c/:slug" element={<PersistedConversation />} />
      <Route path="/product-conversations/:slug" element={<PersistedConversation />} />
      <Route path="/chains/:slug" element={<PersistedConversation />} />
      <Route path="/global/:slug" element={<PersistedConversation global />} />
      <Route path="*" element={<div role="status">This surface is unavailable in read-only observation.</div>} />
    </Routes>
  </>;
}
