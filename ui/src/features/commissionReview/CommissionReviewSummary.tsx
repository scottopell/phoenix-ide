import { CopyButton } from '../../components/CopyButton';
import type { CommissionReviewDisplayData, CommissionReviewFinding, CommissionReviewInput } from './model';
import type { AgentTextHighlight } from '../../components/MessageComponents';
import './CommissionReviewSummary.css';
import {
  commissionReviewOutcomeClass,
  commissionReviewOutcomeLabel,
  formatCommissionReviewLabel,
  severityRank,
  buildCommissionReviewInlineSearchFragments,
} from './model';

function highlightCommissionText(text: string, highlight: AgentTextHighlight) {
  if (highlight.start < 0 || highlight.end <= highlight.start || highlight.start >= text.length) return text;
  const end = Math.min(highlight.end, text.length);
  return <>{text.slice(0, highlight.start)}<mark className="viewer-find-inline-match viewer-find-inline-match--active">{text.slice(highlight.start, end)}</mark>{text.slice(end)}</>;
}

const COMMISSION_REVIEW_FINDINGS_PREVIEW_LIMIT = 5;

interface CommissionReviewInputViewProps {
  input: CommissionReviewInput;
  activeHighlight?: AgentTextHighlight | null | undefined;
}

export function CommissionReviewInputView({ input, activeHighlight = null }: CommissionReviewInputViewProps) {
  const semanticText = `brief: ${input.brief}${input.focus ? `\nfocus: ${input.focus}` : ''}`;
  const visibleInput = activeHighlight
    ? highlightCommissionText(semanticText, activeHighlight)
    : null;
  return (
    <div className="commission-review-input" aria-label="Commission review request" data-fragment-id="tool-use-input">
      {visibleInput ? (
        <div className="commission-review-input-value commission-review-input-value--highlighted">{visibleInput}</div>
      ) : (
        <>
          <div className="commission-review-input-row">
            <span className="commission-review-input-label">brief</span>
            <div className="commission-review-input-value">{input.brief}</div>
          </div>
          {input.focus && (
            <div className="commission-review-input-row">
              <span className="commission-review-input-label">focus</span>
              <div className="commission-review-input-value">{input.focus}</div>
            </div>
          )}
        </>
      )}
    </div>
  );
}

interface CommissionReviewSummaryCardProps {
  data: CommissionReviewDisplayData;
  activeHighlight?: AgentTextHighlight | null | undefined;
  formatDuration: (elapsedMs: number) => string;
  mode?: 'inline' | 'full' | undefined;
  requestSequenceId?: number | undefined;
  onOpenFullReview?: ((requestSequenceId: number) => void) | undefined;
  onCopyRaw?: (() => void) | undefined;
}

export function CommissionReviewSummaryCard({
  data,
  activeHighlight = null,
  formatDuration,
  mode = 'inline',
  requestSequenceId,
  onOpenFullReview,
  onCopyRaw,
}: CommissionReviewSummaryCardProps) {
  const outcomeClass = commissionReviewOutcomeClass(data);
  const outcomeLabel = commissionReviewOutcomeLabel(data);
  const showFullDetails = mode === 'full';
  const canOpenFullReview = !showFullDetails && requestSequenceId !== undefined && onOpenFullReview !== undefined;
  const renderAllDetails = showFullDetails || !canOpenFullReview;
  const findingsPreview = [...data.findings]
    .sort((a, b) => severityRank(a.severity) - severityRank(b.severity) || a.file.localeCompare(b.file) || (a.line ?? Number.MAX_SAFE_INTEGER) - (b.line ?? Number.MAX_SAFE_INTEGER))
    .slice(0, renderAllDetails ? data.findings.length : COMMISSION_REVIEW_FINDINGS_PREVIEW_LIMIT);
  const remainingFindings = Math.max(0, data.findings.length - findingsPreview.length);
  const severityBadges = ('findingSummary' in data
    ? ([
        ['critical', data.findingSummary.critical],
        ['high', data.findingSummary.high],
        ['medium', data.findingSummary.medium],
        ['low', data.findingSummary.low],
      ] satisfies Array<[CommissionReviewFinding['severity'], number]>)
    : []).filter((entry) => entry[1] > 0);
  const coverageLabel = 'summary' in data ? `${data.summary.filesReviewed}/${data.summary.filesChanged} files reviewed` : null;
  const stageDetails = [
    ['target collection', data.stageStatus.targetCollection],
    ['diff collection', data.stageStatus.diffCollection],
    ['llm review', data.stageStatus.llmReview],
    ['json parse', data.stageStatus.jsonParse],
    ['finding extraction', data.stageStatus.findingExtraction],
  ] as const;
  const findingRationaleFallback = (finding: CommissionReviewFinding) => {
    return finding.rationale.length > 0 ? finding.rationale : 'No rationale provided by the reviewer.';
  };
  const highlightedFragment = (fragmentId: string, text: string) => activeHighlight?.fragmentId === fragmentId
    ? highlightCommissionText(text, activeHighlight)
    : text;
  const searchableFragments = new Map(buildCommissionReviewInlineSearchFragments(data, { renderAllDetails }).map((fragment) => [fragment.fragmentId, fragment.text]));
  return (
    <section className={`commission-review-result ${outcomeClass} ${showFullDetails ? 'commission-review-result--full' : ''}`} aria-label="Commission review summary">
      <div className="commission-review-summary-header">
        <div>
          <div className="commission-review-summary-title" data-fragment-id="commission-review-header">
            {activeHighlight?.fragmentId === 'commission-review-header' ? (
              highlightCommissionText(searchableFragments.get('commission-review-header') ?? '', activeHighlight)
            ) : (<>
              Commission review
              <div className="commission-review-summary-subtitle">
                {formatCommissionReviewLabel(data.reviewStatus)} · trust {formatCommissionReviewLabel(data.findingsTrust)}
              </div>
            </>)}
          </div>
        </div>
        <div className="commission-review-summary-header-actions">
          {canOpenFullReview && (
            <button
              type="button"
              className="commission-review-open-button"
              onClick={() => onOpenFullReview(requestSequenceId)}
            >
              Open full review
            </button>
          )}
          {onCopyRaw && <CopyButton text={JSON.stringify(data, null, 2)} title="Copy parsed review data" />}
          <div className={`commission-review-outcome-pill ${outcomeClass}`} data-fragment-id="commission-review-outcome">
            {highlightedFragment('commission-review-outcome', outcomeLabel)}
          </div>
        </div>
      </div>

      {data.status === 'rejected' && data.reviewerSummary && (
        <p className="commission-review-reviewer-summary" data-fragment-id="commission-review-summary">
          {highlightedFragment('commission-review-summary', data.reviewerSummary)}
        </p>
      )}

      {'summary' in data && 'findingSummary' in data && (
        <>
          <div className="commission-review-metrics" role="list" aria-label="Commission review metrics">
            <div className="commission-review-metric" role="listitem" data-fragment-id="commission-review-elapsed">{highlightedFragment('commission-review-elapsed', `elapsed\n${formatDuration(data.summary.elapsedMs)}`)}</div>
            <div className="commission-review-metric" role="listitem" data-fragment-id="commission-review-coverage">
              {activeHighlight?.fragmentId === 'commission-review-coverage'
                ? highlightCommissionText(`coverage\n${coverageLabel}`, activeHighlight)
                : <><span>coverage</span><strong>{coverageLabel}</strong></>}
            </div>
            <div className="commission-review-metric" role="listitem" data-fragment-id="commission-review-changes">{highlightedFragment('commission-review-changes', `changes\n+${data.summary.insertions} / -${data.summary.deletions}`)}</div>
            <div className="commission-review-metric" role="listitem" data-fragment-id="commission-review-total">{highlightedFragment('commission-review-total', `findings\n${data.findingSummary.total}`)}</div>
          </div>

          <div className="commission-review-target" data-fragment-id="commission-review-target">
            {activeHighlight?.fragmentId === 'commission-review-target'
              ? highlightCommissionText(`${data.summary.target.base} → ${data.summary.target.head}\n${data.summary.target.repoRoot}`, activeHighlight)
              : <><div className="commission-review-target-branch">{data.summary.target.base} → {data.summary.target.head}</div><div className="commission-review-target-repo">{data.summary.target.repoRoot}</div></>}
          </div>

          {data.summary.reviewerSummary && (
            <p className="commission-review-reviewer-summary" data-fragment-id="commission-review-summary">
              {highlightedFragment('commission-review-summary', data.summary.reviewerSummary)}
            </p>
          )}
        </>
      )}

      {severityBadges.length > 0 && (
        <div className="commission-review-severity-row" aria-label="Finding counts by severity" data-fragment-id="commission-review-severities">
          {activeHighlight?.fragmentId === 'commission-review-severities'
            ? highlightCommissionText(searchableFragments.get('commission-review-severities') ?? '', activeHighlight)
            : severityBadges.map(([severity, count]) => (
              <span key={severity} className={`commission-review-severity-badge ${severity}`}>
                {severity} {count}
              </span>
            ))}
        </div>
      )}

      {(data.warningsSummary.length > 0 || data.unreviewed.length > 0 || data.retryRecommendation !== 'do_not_retry') && (
        <div className="commission-review-callouts">
          {data.warningsSummary.length > 0 && (
            <div className="commission-review-callout warning">
              <div className="commission-review-callout-title">Warnings</div>
              <ul>
                {data.warningsSummary.map((warning, index) => (
                  <li key={warning} data-fragment-id={`commission-review-warning-${index}`}>
                    {highlightedFragment(`commission-review-warning-${index}`, warning)}
                  </li>
                ))}
              </ul>
            </div>
          )}
          {data.unreviewed.length > 0 && (
            <div className="commission-review-callout coverage-gap">
              <div className="commission-review-callout-title">Unreviewed files</div>
              <ul>
                {(renderAllDetails ? data.unreviewed : data.unreviewed.slice(0, 3)).map((entry, index) => {
                  const text = `${entry.file}${entry.reason ? ` · ${formatCommissionReviewLabel(entry.reason)}` : ''}`;
                  return <li key={`${entry.file}-${entry.reason ?? 'na'}`} data-fragment-id={`commission-review-unreviewed-${index}`}>{highlightedFragment(`commission-review-unreviewed-${index}`, text)}</li>;
                })}
              </ul>
              {!renderAllDetails && data.unreviewed.length > 3 && <div className="commission-review-more">+{data.unreviewed.length - 3} more files</div>}
            </div>
          )}
          {data.retryRecommendation !== 'do_not_retry' && (
            <div className="commission-review-callout retry" data-fragment-id="commission-review-retry">
              {activeHighlight?.fragmentId === 'commission-review-retry'
                ? highlightCommissionText(searchableFragments.get('commission-review-retry') ?? '', activeHighlight)
                : <><div className="commission-review-callout-title">Retry guidance</div><p>{formatCommissionReviewLabel(data.retryRecommendation)}</p></>}
            </div>
          )}
        </div>
      )}

      {renderAllDetails && (
        <div className="commission-review-status-grid" aria-label="Commission review status details" data-fragment-id="commission-review-status-details">
          {activeHighlight?.fragmentId === 'commission-review-status-details' ? (
            highlightCommissionText(searchableFragments.get('commission-review-status-details') ?? '', activeHighlight)
          ) : <>
            <div><span>status</span><strong>{formatCommissionReviewLabel(data.status)}</strong></div>
            <div><span>review</span><strong>{formatCommissionReviewLabel(data.reviewStatus)}</strong></div>
            <div><span>findings</span><strong>{formatCommissionReviewLabel(data.findingsStatus)}</strong></div>
            <div><span>trust</span><strong>{formatCommissionReviewLabel(data.findingsTrust)}</strong></div>
            <div><span>retry</span><strong>{formatCommissionReviewLabel(data.retryRecommendation)}</strong></div>
            {stageDetails.map(([label, value]) => (
              <div key={label}><span>{label}</span><strong>{value ? formatCommissionReviewLabel(value) : 'not reported'}</strong></div>
            ))}
            {'summary' in data && (
              <>
                <div><span>target kind</span><strong>{data.summary.target.kind ? formatCommissionReviewLabel(data.summary.target.kind) : 'unknown'}</strong></div>
                <div><span>dirty</span><strong>{data.summary.target.dirty ? 'yes' : 'no'}</strong></div>
              </>
            )}
          </>}
        </div>
      )}

      {renderAllDetails && data.warnings.length > 0 && (
        <div className="commission-review-detail-block" aria-label="Commission review detailed warnings">
          <div className="commission-review-findings-header">Detailed warnings</div>
          <ul className="commission-review-warning-list">
            {data.warnings.map((warning, index) => (
              <li key={`${warning.message}-${index}`} data-fragment-id={`commission-review-detail-warning-${index}`}>
                {activeHighlight?.fragmentId === `commission-review-detail-warning-${index}`
                  ? highlightCommissionText(searchableFragments.get(`commission-review-detail-warning-${index}`) ?? '', activeHighlight)
                  : <><strong>{warning.kind ? formatCommissionReviewLabel(warning.kind) : 'warning'}</strong><span>{warning.message}</span>{warning.file && <code>{warning.file}</code>}</>}
              </li>
            ))}
          </ul>
        </div>
      )}

      {findingsPreview.length > 0 ? (
        <div className="commission-review-findings">
          <div className="commission-review-findings-header">{renderAllDetails ? 'Findings' : 'Top findings'}</div>
          <ol>
            {findingsPreview.map((finding, index) => (
              <li
                key={`${finding.file}-${finding.line ?? 'na'}-${finding.title}-${index}`}
                className={`commission-review-finding ${finding.severity}`}
                data-fragment-id={`commission-review-finding-${index}`}
              >
                {activeHighlight?.fragmentId === `commission-review-finding-${index}` ? (
                  highlightCommissionText(searchableFragments.get(`commission-review-finding-${index}`) ?? '', activeHighlight)
                ) : (<>
                <div className="commission-review-finding-header">
                  <span className={`commission-review-severity-badge ${finding.severity}`}>{finding.severity}</span>
                  <strong>{finding.title}</strong>
                  {finding.confidence && <span className="commission-review-finding-confidence">{formatCommissionReviewLabel(finding.confidence)} confidence</span>}
                </div>
                <div className="commission-review-finding-location">
                  {finding.file}
                  {finding.line !== undefined ? `:${finding.line}` : ''}
                  {finding.symbol ? ` · ${finding.symbol}` : ''}
                </div>
                <p>{findingRationaleFallback(finding)}</p>
                {finding.suggestedFix && <p className="commission-review-suggested-fix">Fix: {finding.suggestedFix}</p>}
                </>)}
              </li>
            ))}
          </ol>
          {remainingFindings > 0 && <div className="commission-review-more">+{remainingFindings} more findings not shown</div>}
        </div>
      ) : (
        <div className="commission-review-empty-findings">
          {data.status === 'failed' ? 'No actionable findings were produced.' : data.status === 'rejected' ? 'Review was rejected before findings were produced.' : 'No findings reported.'}
        </div>
      )}
    </section>
  );
}
