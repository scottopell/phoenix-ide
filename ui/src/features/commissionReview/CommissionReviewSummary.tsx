import { CopyButton } from '../../components/CopyButton';
import type { CommissionReviewDisplayData, CommissionReviewFinding, CommissionReviewInput } from './model';
import {
  commissionReviewOutcomeClass,
  commissionReviewOutcomeLabel,
  formatCommissionReviewLabel,
  severityRank,
} from './model';

const COMMISSION_REVIEW_FINDINGS_PREVIEW_LIMIT = 5;

interface CommissionReviewInputViewProps {
  input: CommissionReviewInput;
}

export function CommissionReviewInputView({ input }: CommissionReviewInputViewProps) {
  return (
    <div className="commission-review-input" aria-label="Commission review request">
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
    </div>
  );
}

interface CommissionReviewSummaryCardProps {
  data: CommissionReviewDisplayData;
  formatDuration: (elapsedMs: number) => string;
  mode?: 'inline' | 'full' | undefined;
  requestSequenceId?: number | undefined;
  onOpenFullReview?: ((requestSequenceId: number) => void) | undefined;
  onCopyRaw?: (() => void) | undefined;
}

export function CommissionReviewSummaryCard({
  data,
  formatDuration,
  mode = 'inline',
  requestSequenceId,
  onOpenFullReview,
  onCopyRaw,
}: CommissionReviewSummaryCardProps) {
  const outcomeClass = commissionReviewOutcomeClass(data);
  const outcomeLabel = commissionReviewOutcomeLabel(data);
  const findingsPreview = [...data.findings]
    .sort((a, b) => severityRank(a.severity) - severityRank(b.severity) || a.file.localeCompare(b.file) || (a.line ?? Number.MAX_SAFE_INTEGER) - (b.line ?? Number.MAX_SAFE_INTEGER))
    .slice(0, mode === 'inline' ? COMMISSION_REVIEW_FINDINGS_PREVIEW_LIMIT : data.findings.length);
  const remainingFindings = Math.max(0, data.findings.length - findingsPreview.length);
  const severityBadges = ([
    ['critical', data.findingSummary.critical],
    ['high', data.findingSummary.high],
    ['medium', data.findingSummary.medium],
    ['low', data.findingSummary.low],
  ] satisfies Array<[CommissionReviewFinding['severity'], number]>).filter((entry) => entry[1] > 0);
  const coverageLabel = `${data.summary.filesReviewed}/${data.summary.filesChanged} files reviewed`;
  const showFullDetails = mode === 'full';
  const canOpenFullReview = !showFullDetails && requestSequenceId !== undefined && onOpenFullReview !== undefined;

  return (
    <section className={`commission-review-result ${outcomeClass} ${showFullDetails ? 'commission-review-result--full' : ''}`} aria-label="Commission review summary">
      <div className="commission-review-summary-header">
        <div>
          <div className="commission-review-summary-title">Commission review</div>
          <div className="commission-review-summary-subtitle">
            {formatCommissionReviewLabel(data.reviewStatus)} · trust {formatCommissionReviewLabel(data.findingsTrust)}
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
          <div className={`commission-review-outcome-pill ${outcomeClass}`}>{outcomeLabel}</div>
        </div>
      </div>

      <div className="commission-review-metrics" role="list" aria-label="Commission review metrics">
        <div className="commission-review-metric" role="listitem"><span>elapsed</span><strong>{formatDuration(data.summary.elapsedMs)}</strong></div>
        <div className="commission-review-metric" role="listitem"><span>coverage</span><strong>{coverageLabel}</strong></div>
        <div className="commission-review-metric" role="listitem"><span>changes</span><strong>+{data.summary.insertions} / -{data.summary.deletions}</strong></div>
        <div className="commission-review-metric" role="listitem"><span>findings</span><strong>{data.findingSummary.total}</strong></div>
      </div>

      <div className="commission-review-target">
        <div className="commission-review-target-branch">{data.summary.target.base} → {data.summary.target.head}</div>
        <div className="commission-review-target-repo">{data.summary.target.repoRoot}</div>
      </div>

      {data.summary.reviewerSummary && (
        <p className="commission-review-reviewer-summary">{data.summary.reviewerSummary}</p>
      )}

      {severityBadges.length > 0 && (
        <div className="commission-review-severity-row" aria-label="Finding counts by severity">
          {severityBadges.map(([severity, count]) => (
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
                {data.warningsSummary.map((warning) => <li key={warning}>{warning}</li>)}
              </ul>
            </div>
          )}
          {data.unreviewed.length > 0 && (
            <div className="commission-review-callout coverage-gap">
              <div className="commission-review-callout-title">Unreviewed files</div>
              <ul>
                {(showFullDetails ? data.unreviewed : data.unreviewed.slice(0, 3)).map((entry) => <li key={`${entry.file}-${entry.reason ?? 'na'}`}>{entry.file}{entry.reason ? ` · ${formatCommissionReviewLabel(entry.reason)}` : ''}</li>)}
              </ul>
              {!showFullDetails && data.unreviewed.length > 3 && <div className="commission-review-more">+{data.unreviewed.length - 3} more files</div>}
            </div>
          )}
          {data.retryRecommendation !== 'do_not_retry' && (
            <div className="commission-review-callout retry">
              <div className="commission-review-callout-title">Retry guidance</div>
              <p>{formatCommissionReviewLabel(data.retryRecommendation)}</p>
            </div>
          )}
        </div>
      )}

      {showFullDetails && (
        <div className="commission-review-status-grid" aria-label="Commission review status details">
          <div><span>status</span><strong>{formatCommissionReviewLabel(data.status)}</strong></div>
          <div><span>review</span><strong>{formatCommissionReviewLabel(data.reviewStatus)}</strong></div>
          <div><span>findings</span><strong>{formatCommissionReviewLabel(data.findingsStatus)}</strong></div>
          <div><span>retry</span><strong>{formatCommissionReviewLabel(data.retryRecommendation)}</strong></div>
          <div><span>target kind</span><strong>{data.summary.target.kind ? formatCommissionReviewLabel(data.summary.target.kind) : 'unknown'}</strong></div>
          <div><span>dirty</span><strong>{data.summary.target.dirty ? 'yes' : 'no'}</strong></div>
        </div>
      )}

      {showFullDetails && data.warnings.length > 0 && (
        <div className="commission-review-detail-block" aria-label="Commission review detailed warnings">
          <div className="commission-review-findings-header">Detailed warnings</div>
          <ul className="commission-review-warning-list">
            {data.warnings.map((warning, index) => (
              <li key={`${warning.message}-${index}`}>
                <strong>{warning.kind ? formatCommissionReviewLabel(warning.kind) : 'warning'}</strong>
                <span>{warning.message}</span>
                {warning.file && <code>{warning.file}</code>}
              </li>
            ))}
          </ul>
        </div>
      )}

      {findingsPreview.length > 0 ? (
        <div className="commission-review-findings">
          <div className="commission-review-findings-header">{showFullDetails ? 'Findings' : 'Top findings'}</div>
          <ol>
            {findingsPreview.map((finding, index) => (
              <li key={`${finding.file}-${finding.line ?? 'na'}-${finding.title}-${index}`} className={`commission-review-finding ${finding.severity}`}>
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
                <p>{finding.rationale}</p>
                {finding.suggestedFix && <p className="commission-review-suggested-fix">Fix: {finding.suggestedFix}</p>}
              </li>
            ))}
          </ol>
          {remainingFindings > 0 && <div className="commission-review-more">+{remainingFindings} more findings not shown</div>}
        </div>
      ) : (
        <div className="commission-review-empty-findings">
          {data.status === 'failed' ? 'No actionable findings were produced.' : 'No findings reported.'}
        </div>
      )}
    </section>
  );
}
