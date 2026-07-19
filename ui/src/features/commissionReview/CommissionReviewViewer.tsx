import type { Message, ToolResultContent } from '../../api';
import { ViewerPresentationControl, ViewerShell } from '../../components/viewer/ViewerShell';
import { useRegisterFocusScope } from '../../hooks/useFocusScope';
import { CommissionReviewSummaryCard } from './CommissionReviewSummary';
import { parseCommissionReviewResult } from './model';
import './CommissionReviewViewer.css';

interface CommissionReviewViewerProps {
  sequenceId: number;
  messages: Message[];
  onClose: () => void;
  presentation?: 'pane' | 'fullscreen' | undefined;
  canTogglePresentation?: boolean | undefined;
  onPresentationChange?: ((presentation: 'pane' | 'fullscreen') => void) | undefined;
  inline?: boolean | undefined;
}

interface ResolvedReview {
  requestMessage: Message;
  resultMessage: Message | null;
  resultContent: ToolResultContent | null;
}

export function CommissionReviewViewer({ sequenceId, messages, onClose, presentation = 'pane', canTogglePresentation = false, onPresentationChange, inline }: CommissionReviewViewerProps) {
  useRegisterFocusScope('commission-review-viewer');
  const resolved = resolveCommissionReviewBySequence(sequenceId, messages);
  const rawResult = resolved?.resultContent?.content ?? resolved?.resultContent?.result ?? resolved?.resultContent?.error ?? '';
  const parsed = resolved?.resultMessage ? parseCommissionReviewResult(resolved.resultMessage.display_data, rawResult) : null;

  const focused = presentation === 'fullscreen' && canTogglePresentation;
  const shell = (
    <ViewerShell
      mode={focused ? 'takeover' : inline ? 'inline' : 'overlay'}
      ariaLabel={`Commission review viewer: #${sequenceId}`}
      title={resolved ? `Commission review #${resolved.requestMessage.sequence_id}` : `Commission review #${sequenceId}`}
      titleTooltip={resolved?.resultMessage ? `Tool result message #${resolved.resultMessage.sequence_id}` : undefined}
      headerExtras={canTogglePresentation && onPresentationChange ? (
        <ViewerPresentationControl
          fullscreen={focused}
          onToggle={() => onPresentationChange(focused ? 'pane' : 'fullscreen')}
        />
      ) : null}
      noteCount={0}
      onToggleNotes={() => {}}
      onSend={() => {}}
      onClose={onClose}
      onEscape={focused && onPresentationChange ? () => onPresentationChange('pane') : undefined}
      bodyScroll="shell"
    >
      <div className="viewer-content commission-review-viewer">
        {!resolved ? (
          <div className="commission-review-viewer-state" aria-label="Commission review missing">
            <h2>Commission review not found</h2>
            <p>No commission_review tool invocation matched transcript sequence #{sequenceId}.</p>
          </div>
        ) : !resolved.resultMessage || !resolved.resultContent ? (
          <div className="commission-review-viewer-state" aria-label="Commission review result missing">
            <h2>Review result missing</h2>
            <p>The commission_review request exists, but no finalized tool result message matched its tool_use id.</p>
          </div>
        ) : !parsed ? (
          <div className="commission-review-viewer-state" aria-label="Commission review malformed">
            <h2>Review data malformed</h2>
            <p>This tool result exists, but its display_data payload was missing or malformed.</p>
            {rawResult && (
              <pre className="commission-review-viewer-raw">{rawResult}</pre>
            )}
          </div>
        ) : (
          <CommissionReviewSummaryCard data={parsed} formatDuration={formatDuration} mode="full" />
        )}
      </div>
    </ViewerShell>
  );
  return shell;
}

function resolveCommissionReviewBySequence(sequenceId: number, messages: Message[]): ResolvedReview | null {
  const requestMessage = messages.find((message) => message.sequence_id === sequenceId);
  if (!requestMessage || !Array.isArray(requestMessage.content)) return null;

  const toolUse = requestMessage.content.find((block) => block.type === 'tool_use' && block.name === 'commission_review');
  if (!toolUse?.id) return null;

  const resultMessage = messages.find((message) => {
    if (message.message_type !== 'tool') return false;
    const content = message.content as ToolResultContent;
    return content?.tool_use_id === toolUse.id;
  }) ?? null;

  return {
    requestMessage,
    resultMessage,
    resultContent: resultMessage ? resultMessage.content as ToolResultContent : null,
  };
}

function formatDuration(elapsedMs: number): string {
  if (elapsedMs < 1000) return `${elapsedMs}ms`;
  const seconds = elapsedMs / 1000;
  return seconds >= 10 ? `${Math.round(seconds)}s` : `${seconds.toFixed(1)}s`;
}
