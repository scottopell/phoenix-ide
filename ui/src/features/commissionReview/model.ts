export type CommissionReviewInput = {
  brief: string;
  focus?: string | undefined;
};

export type CommissionReviewFindingSeverity = 'critical' | 'high' | 'medium' | 'low';

export type CommissionReviewFinding = {
  severity: CommissionReviewFindingSeverity;
  confidence?: string | undefined;
  file: string;
  line?: number | undefined;
  symbol?: string | undefined;
  title: string;
  rationale: string;
  suggestedFix?: string | undefined;
};

export type CommissionReviewStageStatus = {
  targetCollection?: string | undefined;
  diffCollection?: string | undefined;
  llmReview?: string | undefined;
  jsonParse?: string | undefined;
  findingExtraction?: string | undefined;
};

export type CommissionReviewResolvedSummary = {
  target: { kind?: string | undefined; repoRoot: string; base: string; head: string; dirty?: boolean | undefined };
  filesChanged: number;
  filesReviewed: number;
  insertions: number;
  deletions: number;
  elapsedMs: number;
  reviewerSummary?: string | undefined;
};

export type CommissionReviewResolvedDisplayData = {
  kind: 'commission_review';
  status: 'success' | 'partial' | 'failed' | 'skipped';
  reviewStatus: string;
  findingsStatus: string;
  findingsTrust: string;
  retryRecommendation: string;
  stageStatus: CommissionReviewStageStatus;
  findingSummary: { total: number; critical: number; high: number; medium: number; low: number };
  warningsSummary: string[];
  summary: CommissionReviewResolvedSummary;
  unreviewed: Array<{ file: string; reason?: string | undefined }>;
  findings: CommissionReviewFinding[];
  warnings: Array<{ kind?: string | undefined; message: string; file?: string | undefined }>;
};

export type CommissionReviewRejectedDisplayData = {
  kind: 'commission_review';
  status: 'rejected';
  reviewStatus: 'rejected';
  findingsStatus: 'unavailable';
  findingsTrust: 'low';
  retryRecommendation: 'do_not_retry';
  stageStatus: CommissionReviewStageStatus;
  warningsSummary: string[];
  reviewerSummary?: string | undefined;
  unreviewed: Array<{ file: string; reason?: string | undefined }>;
  findings: CommissionReviewFinding[];
  warnings: Array<{ kind?: string | undefined; message: string; file?: string | undefined }>;
};

export type CommissionReviewDisplayData = CommissionReviewResolvedDisplayData | CommissionReviewRejectedDisplayData;

function asRecord(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? value as Record<string, unknown> : null;
}

function asString(value: unknown): string | undefined {
  return typeof value === 'string' && value.trim().length > 0 ? value.trim() : undefined;
}

function asOptionalString(value: unknown): string | undefined {
  return typeof value === 'string' ? value.trim() : undefined;
}

function asNumber(value: unknown): number | undefined {
  return typeof value === 'number' && Number.isFinite(value) ? value : undefined;
}

function asStringArray(value: unknown): string[] {
  return Array.isArray(value) ? value.map(asString).filter((v): v is string => !!v) : [];
}

export function formatCommissionReviewLabel(value: string): string {
  return value.replace(/_/g, ' ');
}

export function severityRank(severity: CommissionReviewFindingSeverity): number {
  switch (severity) {
    case 'critical': return 0;
    case 'high': return 1;
    case 'medium': return 2;
    case 'low': return 3;
  }
}

export function formatCommissionReviewInput(input: Record<string, unknown>): { display: string; isMultiline: boolean } {
  const brief = asString(input['brief']) ?? '<missing brief>';
  const focus = asString(input['focus']);
  const lines = [`brief: ${brief}`];
  if (focus) lines.push(`focus: ${focus}`);
  return { display: lines.join('\n'), isMultiline: true };
}

export function parseCommissionReviewInput(input: Record<string, unknown>): CommissionReviewInput | null {
  const brief = asString(input['brief']);
  if (!brief) return null;
  return { brief, focus: asString(input['focus']) };
}

function parseStageStatus(value: unknown): CommissionReviewStageStatus {
  const record = asRecord(value);
  if (!record) return {};
  return {
    targetCollection: asString(record['target_collection']),
    diffCollection: asString(record['diff_collection']),
    llmReview: asString(record['llm_review']),
    jsonParse: asString(record['json_parse']),
    findingExtraction: asString(record['finding_extraction']),
  };
}

function parseFindings(value: unknown): CommissionReviewFinding[] {
  const findings: CommissionReviewFinding[] = [];
  if (!Array.isArray(value)) return findings;
  for (const entry of value) {
    const item = asRecord(entry);
    if (!item) continue;
    const severity = asString(item['severity']);
    const file = asString(item['file']);
    const title = asString(item['title']);
    const rationale = asOptionalString(item['rationale']);
    if (!severity || !file || !title || rationale === undefined) continue;
    if (!['critical', 'high', 'medium', 'low'].includes(severity)) continue;
    findings.push({
      severity: severity as CommissionReviewFindingSeverity,
      confidence: asString(item['confidence']),
      file,
      line: asNumber(item['line']),
      symbol: asString(item['symbol']),
      title,
      rationale,
      suggestedFix: asString(item['suggested_fix']),
    });
  }
  return findings;
}

function parseUnreviewed(value: unknown): CommissionReviewDisplayData['unreviewed'] {
  const unreviewed: CommissionReviewDisplayData['unreviewed'] = [];
  if (Array.isArray(value)) {
    for (const entry of value) {
      const item = asRecord(entry);
      const file = asString(item?.['file']);
      if (file) unreviewed.push({ file, reason: asString(item?.['reason']) });
    }
  }
  return unreviewed;
}

function parseWarnings(value: unknown): CommissionReviewDisplayData['warnings'] {
  const warnings: CommissionReviewDisplayData['warnings'] = [];
  if (Array.isArray(value)) {
    for (const entry of value) {
      const item = asRecord(entry);
      const message = asString(item?.['message']);
      if (message) warnings.push({ kind: asString(item?.['kind']), message, file: asString(item?.['file']) });
    }
  }
  return warnings;
}

export function parseCommissionReviewDisplayData(value: unknown): CommissionReviewDisplayData | null {
  const record = asRecord(value);
  if (!record || record['kind'] !== 'commission_review') return null;

  const status = asString(record['status']);
  const validStatuses: CommissionReviewDisplayData['status'][] = ['success', 'partial', 'failed', 'skipped', 'rejected'];
  if (!status || !validStatuses.includes(status as CommissionReviewDisplayData['status'])) return null;

  const common = {
    kind: 'commission_review' as const,
    stageStatus: parseStageStatus(record['stage_status']),
    warningsSummary: asStringArray(record['warnings_summary']),
    unreviewed: parseUnreviewed(record['unreviewed']),
    findings: parseFindings(record['findings']),
    warnings: parseWarnings(record['warnings']),
  };

  if (status === 'rejected') {
    const summary = asRecord(record['summary']);
    return {
      ...common,
      status,
      reviewStatus: 'rejected',
      findingsStatus: 'unavailable',
      findingsTrust: 'low',
      retryRecommendation: 'do_not_retry',
      reviewerSummary: asString(summary?.['reviewer_summary']),
    };
  }

  const reviewStatus = asString(record['review_status']);
  const findingsStatus = asString(record['findings_status']);
  const findingsTrust = asString(record['findings_trust']);
  const retryRecommendation = asString(record['retry_recommendation']);
  if (!reviewStatus || !findingsStatus || !findingsTrust || !retryRecommendation) return null;

  const resolvedCommon = {
    ...common,
    status,
    reviewStatus,
    findingsStatus,
    findingsTrust,
    retryRecommendation,
  };

  const summary = asRecord(record['summary']);
  const target = asRecord(summary?.['target']);
  const findingSummary = asRecord(record['finding_summary']);
  if (!summary || !target || !findingSummary) return null;

  const repoRoot = asString(target['repo_root']);
  const base = asString(target['base']);
  const head = asString(target['head']);
  const filesChanged = asNumber(summary['files_changed']);
  const filesReviewed = asNumber(summary['files_reviewed']);
  const insertions = asNumber(summary['insertions']);
  const deletions = asNumber(summary['deletions']);
  const elapsedMs = asNumber(summary['elapsed_ms']);
  const total = asNumber(findingSummary['total']);
  const critical = asNumber(findingSummary['critical']);
  const high = asNumber(findingSummary['high']);
  const medium = asNumber(findingSummary['medium']);
  const low = asNumber(findingSummary['low']);

  if (!repoRoot || !base || !head) return null;
  if ([filesChanged, filesReviewed, insertions, deletions, elapsedMs, total, critical, high, medium, low].some((v) => v === undefined)) return null;

  return {
    ...resolvedCommon,
    status: status as CommissionReviewResolvedDisplayData['status'],
    findingSummary: { total: total!, critical: critical!, high: high!, medium: medium!, low: low! },
    summary: {
      target: { kind: asString(target['kind']), repoRoot, base, head, dirty: target['dirty'] === true },
      filesChanged: filesChanged!,
      filesReviewed: filesReviewed!,
      insertions: insertions!,
      deletions: deletions!,
      elapsedMs: elapsedMs!,
      reviewerSummary: asString(summary['reviewer_summary']),
    },
  };
}

export function parseCommissionReviewResult(displayData: unknown, rawContent: string): CommissionReviewDisplayData | null {
  const structured = parseCommissionReviewDisplayData(displayData);
  if (structured) return structured;

  let raw: unknown;
  try {
    raw = JSON.parse(rawContent);
  } catch {
    return null;
  }
  const record = asRecord(raw);
  if (!record || record['status'] !== 'rejected') return null;
  const summary = asRecord(record['summary']);
  if (!summary || !Array.isArray(record['findings']) || !Array.isArray(record['warnings'])) return null;

  return {
    kind: 'commission_review',
    status: 'rejected',
    reviewStatus: 'rejected',
    findingsStatus: 'unavailable',
    findingsTrust: 'low',
    retryRecommendation: 'do_not_retry',
    stageStatus: {},
    warningsSummary: [],
    reviewerSummary: asString(summary['reviewer_summary']),
    unreviewed: [],
    findings: parseFindings(record['findings']),
    warnings: parseWarnings(record['warnings']),
  };
}

export function formatCommissionReviewDuration(elapsedMs: number): string {
  if (elapsedMs < 1000) return `${Math.round(elapsedMs)}ms`;
  const totalSeconds = Math.floor(elapsedMs / 1000);
  if (totalSeconds < 10) return `${(elapsedMs / 1000).toFixed(1)}s`;
  if (totalSeconds < 60) return `${totalSeconds}s`;
  const minutes = Math.floor(totalSeconds / 60);
  const remainingSeconds = totalSeconds % 60;
  return remainingSeconds > 0 ? `${minutes}m ${remainingSeconds}s` : `${minutes}m`;
}

export type CommissionReviewSearchFragment = { fragmentId: string; text: string };

export function buildCommissionReviewInlineSearchFragments(
  data: CommissionReviewDisplayData,
  options: { renderAllDetails?: boolean } = {},
): CommissionReviewSearchFragment[] {
  const renderAllDetails = options.renderAllDetails === true;
  const fragments: CommissionReviewSearchFragment[] = [
    { fragmentId: 'commission-review-header', text: `Commission review\n${formatCommissionReviewLabel(data.reviewStatus)} · trust ${formatCommissionReviewLabel(data.findingsTrust)}` },
    { fragmentId: 'commission-review-outcome', text: commissionReviewOutcomeLabel(data) },
  ];
  if ('summary' in data) {
    fragments.push(
      { fragmentId: 'commission-review-elapsed', text: `elapsed\n${formatCommissionReviewDuration(data.summary.elapsedMs)}` },
      { fragmentId: 'commission-review-coverage', text: `coverage\n${data.summary.filesReviewed}/${data.summary.filesChanged} files reviewed` },
      { fragmentId: 'commission-review-changes', text: `changes\n+${data.summary.insertions} / -${data.summary.deletions}` },
      { fragmentId: 'commission-review-total', text: `findings\n${data.findingSummary.total}` },
      { fragmentId: 'commission-review-target', text: `${data.summary.target.base} → ${data.summary.target.head}\n${data.summary.target.repoRoot}` },
    );
    if (data.summary.reviewerSummary) fragments.push({ fragmentId: 'commission-review-summary', text: data.summary.reviewerSummary });
    const severities = [
      ['critical', data.findingSummary.critical],
      ['high', data.findingSummary.high],
      ['medium', data.findingSummary.medium],
      ['low', data.findingSummary.low],
    ].filter((entry) => Number(entry[1]) > 0).map(([severity, count]) => `${severity} ${count}`);
    if (severities.length > 0) fragments.push({ fragmentId: 'commission-review-severities', text: severities.join('\n') });
  } else if (data.reviewerSummary) {
    fragments.push({ fragmentId: 'commission-review-summary', text: data.reviewerSummary });
  }
  if (data.retryRecommendation !== 'do_not_retry') {
    fragments.push({
      fragmentId: 'commission-review-retry',
      text: `Retry guidance\n${formatCommissionReviewLabel(data.retryRecommendation)}`,
    });
  }
  data.warningsSummary.forEach((warning, index) => fragments.push({ fragmentId: `commission-review-warning-${index}`, text: warning }));
  (renderAllDetails ? data.unreviewed : data.unreviewed.slice(0, 3)).forEach((entry, index) => fragments.push({
    fragmentId: `commission-review-unreviewed-${index}`,
    text: `${entry.file}${entry.reason ? ` · ${formatCommissionReviewLabel(entry.reason)}` : ''}`,
  }));
  [...data.findings]
    .sort((a, b) => severityRank(a.severity) - severityRank(b.severity) || a.file.localeCompare(b.file) || (a.line ?? Number.MAX_SAFE_INTEGER) - (b.line ?? Number.MAX_SAFE_INTEGER))
    .slice(0, renderAllDetails ? data.findings.length : 5)
    .forEach((finding, index) => fragments.push({
      fragmentId: `commission-review-finding-${index}`,
      text: `${finding.severity}\n${finding.title}\n${finding.file}${finding.line !== undefined ? `:${finding.line}` : ''}${finding.symbol ? ` · ${finding.symbol}` : ''}\n${finding.rationale || 'No rationale provided by the reviewer.'}${finding.suggestedFix ? `\nFix: ${finding.suggestedFix}` : ''}`,
    }));
  if (renderAllDetails) {
    const stageDetails: Array<[string, string | undefined]> = [
      ['target collection', data.stageStatus.targetCollection],
      ['diff collection', data.stageStatus.diffCollection],
      ['llm review', data.stageStatus.llmReview],
      ['json parse', data.stageStatus.jsonParse],
      ['finding extraction', data.stageStatus.findingExtraction],
    ];
    fragments.push({
      fragmentId: 'commission-review-status-details',
      text: [
        `status\n${formatCommissionReviewLabel(data.status)}`,
        `review\n${formatCommissionReviewLabel(data.reviewStatus)}`,
        `findings\n${formatCommissionReviewLabel(data.findingsStatus)}`,
        `trust\n${formatCommissionReviewLabel(data.findingsTrust)}`,
        `retry\n${formatCommissionReviewLabel(data.retryRecommendation)}`,
        ...stageDetails.map(([label, value]) => `${label}\n${value ? formatCommissionReviewLabel(value) : 'not reported'}`),
        ...('summary' in data ? [
          `target kind\n${data.summary.target.kind ? formatCommissionReviewLabel(data.summary.target.kind) : 'unknown'}`,
          `dirty\n${data.summary.target.dirty ? 'yes' : 'no'}`,
        ] : []),
      ].join('\n'),
    });
    data.warnings.forEach((warning, index) => fragments.push({
      fragmentId: `commission-review-detail-warning-${index}`,
      text: `${warning.kind ? formatCommissionReviewLabel(warning.kind) : 'warning'}\n${warning.message}${warning.file ? `\n${warning.file}` : ''}`,
    }));
  }
  return fragments;
}

export function commissionReviewOutcomeLabel(data: CommissionReviewDisplayData): string {
  switch (data.status) {
    case 'success': return data.findingSummary.total > 0 ? 'Findings' : 'Clean';
    case 'partial': return 'Partial';
    case 'failed': return 'Failed';
    case 'rejected': return 'Rejected';
    case 'skipped': return 'Skipped';
  }
}

export function commissionReviewOutcomeClass(data: CommissionReviewDisplayData): string {
  switch (data.status) {
    case 'success': return data.findingSummary.total > 0 ? 'has-findings' : 'clean';
    case 'partial': return 'partial';
    case 'failed': return 'failed';
    case 'rejected': return 'rejected';
    case 'skipped': return 'skipped';
  }
}
