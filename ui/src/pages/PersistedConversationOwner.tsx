import { useEffect, useState } from 'react';
import { api } from '../api';
import { useConversationAtom } from '../conversation/useConversationAtom';
import { useApplicationCapabilities } from '../contexts/ApplicationCapabilitiesContext';
import { ConversationNavStack } from '../components/ConversationNavStack';
import { MessageListSkeleton } from '../components/Skeleton';

const noPending: never[] = [];
const noRetry = () => {};

export function PersistedConversationOwner({ slug, showTranscript = true }: { slug: string; showTranscript?: boolean }) {
  const capabilities = useApplicationCapabilities();
  const [atom, dispatch] = useConversationAtom(slug);
  const [status, setStatus] = useState<{ reference: string; error: string | null; loaded: boolean }>({ reference: slug, error: null, loaded: false });
  useEffect(() => {
    setStatus({ reference: slug, error: null, loaded: false });
    if (capabilities.persisted_member_read === 'unavailable') {
      setStatus({ reference: slug, error: 'Persisted conversation observations are unavailable.', loaded: false });
      return;
    }
    const controller = new AbortController();
    let cancelled = false;
    api.getPersistedMember(slug, controller.signal).then((payload) => {
      if (cancelled) return;
      dispatch({ type: 'persisted_hydrated', payload });
      setStatus({ reference: slug, error: null, loaded: true });
    }).catch((error: unknown) => {
      if (!cancelled) setStatus({ reference: slug, error: error instanceof Error ? error.message : String(error), loaded: false });
    });
    return () => { cancelled = true; controller.abort(); };
  }, [capabilities.persisted_member_read, dispatch, slug]);

  if (status.reference !== slug || (!status.loaded && !status.error)) return showTranscript ? <MessageListSkeleton count={6} /> : null;
  if (status.error) return <div role="alert">{status.error}</div>;
  if (atom.observation.source !== 'persisted') return null;
  const observation = atom.observation;
  const member = observation.conversation;
  return <section className={showTranscript ? 'view active' : undefined} data-testid="persisted-conversation-owner">
    <div role="status">
      Persisted content · live connection unavailable. User input unavailable.
      {showTranscript && <> {member.title ?? member.slug ?? member.id} · recorded state: {atom.phase.type.replaceAll('_', ' ')}.</>}
      {observation.persisted_agent_working && <> Recorded agent activity is historical, not a live process observation.</>}
      {atom.transcriptCoverage === 'tail' && <> Showing the newest {atom.messages.length} persisted messages.</>}
      {member.cached_pr && <> Persisted PR: <a href={member.cached_pr.url} target="_blank" rel="noopener noreferrer">#{member.cached_pr.number} {member.cached_pr.title}</a> · {member.cached_pr.display_state}.</>}
    </div>
    {showTranscript && <ConversationNavStack
      messages={atom.messages} pendingMessages={noPending} convState={atom.phase} transcriptMode="persisted"
      onRetry={noRetry} onOpenFile={undefined} enableMessageSidepanel={false} conversationId={member.id}
      filePathRootDir={member.worktree_path ?? member.cwd}
      transcriptPositioning={{ kind: 'idle', view: { conversationId: member.id, generation: 1, transcriptGeneration: atom.transcriptGeneration ?? 0 } }}
    />}
  </section>;
}
