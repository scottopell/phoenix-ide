import { useCallback, useEffect, useMemo, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import {
  ResponsiveContainer,
  AreaChart,
  Area,
  LineChart,
  Line,
  BarChart,
  Bar,
  XAxis,
  YAxis,
  CartesianGrid,
  Tooltip,
  Legend,
} from 'recharts';
import { api, type TrajectoryExportPayload } from '../api';
import type { UsageOverview } from '../generated/UsageOverview';
import type { ConversationUsageDetail } from '../generated/ConversationUsageDetail';
import type { Totals } from '../generated/Totals';
import './UsagePage.css';

function fmtTokens(n: number): string {
  if (n < 1000) return `${Math.round(n)}`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k`;
  if (n < 1_000_000_000) return `${(n / 1_000_000).toFixed(2)}M`;
  return `${(n / 1_000_000_000).toFixed(2)}B`;
}

function fmtUsd(n: number): string {
  if (n === 0) return '$0.00';
  if (n < 1) return `$${n.toFixed(4)}`;
  if (n < 100) return `$${n.toFixed(2)}`;
  return `$${n.toLocaleString(undefined, { maximumFractionDigits: 0 })}`;
}

function fmtLatency(ms: number | null | undefined): string {
  if (ms === null || ms === undefined) return 'unavailable';
  if (ms < 1000) return `${ms.toFixed(0)} ms`;
  return `${(ms / 1000).toFixed(2)} s`;
}

function fmtDateTime(value: string | null | undefined): string {
  if (!value) return 'unavailable';
  const time = Date.parse(value);
  return Number.isNaN(time) ? value : new Date(time).toLocaleString();
}

function totalAnalyticsTokens(tokens: TrajectoryExportPayload['session']['turns'][number]['tokens']): number {
  return tokens.input_tokens + tokens.output_tokens + tokens.cache_creation_tokens + tokens.cache_read_tokens;
}

function fmtCostSummary(cost: Totals['cost']): string {
  const suffix = cost.pricing_known ? '' : '+';
  return `${fmtUsd(cost.estimated_usd)}${suffix}`;
}

function fmtPct(n: number): string {
  return `${(n * 100).toFixed(1)}%`;
}

// Recharts tooltip formatters receive `ValueType | undefined` (number, string,
// or a readonly array); accept `unknown` and normalize to a number.
function numOf(v: unknown): number {
  if (Array.isArray(v)) return Number(v[0]);
  return Number(v ?? 0);
}
const tipTurns = (v: unknown) => `${numOf(v)} turns`;

/** Share of input-side tokens served from cache. */
function cacheHitRate(t: Totals): number {
  const denom = t.input_tokens + t.cache_read_tokens + t.cache_write_tokens;
  return denom > 0 ? t.cache_read_tokens / denom : 0;
}

const TOKEN_SERIES = [
  { key: 'input_tokens', label: 'Input', color: 'var(--accent-blue)' },
  { key: 'output_tokens', label: 'Output', color: 'var(--accent-green)' },
  { key: 'cache_write_tokens', label: 'Cache write', color: 'var(--accent-yellow)' },
  { key: 'cache_read_tokens', label: 'Cache read', color: 'var(--accent-purple)' },
] as const;

const AXIS = { stroke: 'var(--text-muted)', fontSize: 11 };
const GRID = 'var(--border-color)';

function KpiCard({ label, totals }: { label: string; totals: Totals }) {
  return (
    <div className="usage-kpi">
      <div className="usage-kpi__label">{label}</div>
      <div className="usage-kpi__cost">{fmtTokens(totals.total_tokens)}</div>
      <div className="usage-kpi__sub">
        <span>{fmtCostSummary(totals.cost)} est.</span>
        <span>{Math.round(totals.turns)} turns</span>
        <span>{fmtPct(cacheHitRate(totals))} cache</span>
      </div>
    </div>
  );
}

interface TooltipEntry {
  name?: string;
  value?: number;
  color?: string;
  payload?: unknown;
}

function TokenTooltip({ active, payload, label }: { active?: boolean; payload?: TooltipEntry[]; label?: string }) {
  if (!active || !payload?.length) return null;
  const point = payload[0]?.payload as { turnCost?: number | null; pricingKnown?: boolean } | undefined;
  return (
    <div className="usage-tip">
      <div className="usage-tip__title">{label}</div>
      {payload.map((e) => (
        <div key={e.name} className="usage-tip__row">
          <span className="usage-tip__swatch" style={{ background: e.color }} />
          <span>{e.name}</span>
          <span className="usage-tip__val">{fmtTokens(e.value ?? 0)}</span>
        </div>
      ))}
      {point && 'turnCost' in point && (
        <div className="usage-tip__row usage-tip__row--total">
          <span>Estimated cost</span>
          <span className="usage-tip__val">{point.pricingKnown ? fmtUsd(point.turnCost ?? 0) : 'unknown'}</span>
        </div>
      )}
    </div>
  );
}

function CostTooltip({ active, payload, label }: { active?: boolean; payload?: TooltipEntry[]; label?: string }) {
  if (!active || !payload?.length) return null;
  const value = payload[0]?.value ?? 0;
  return (
    <div className="usage-tip">
      <div className="usage-tip__title">Turn {label}</div>
      <div className="usage-tip__row">
        <span>Cumulative estimated cost</span>
        <span className="usage-tip__val">{fmtUsd(value)}</span>
      </div>
    </div>
  );
}

function ChartCard({ title, hint, children }: { title: string; hint?: string; children: React.ReactNode }) {
  return (
    <section className="usage-card">
      <div className="usage-card__head">
        <h3>{title}</h3>
        {hint && <span className="usage-card__hint">{hint}</span>}
      </div>
      <div className="usage-card__body">{children}</div>
    </section>
  );
}


function AnalyticsExportPreview({ id }: { id: string }) {
  const [open, setOpen] = useState(false);
  const [payload, setPayload] = useState<TrajectoryExportPayload | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [copied, setCopied] = useState(false);

  const load = useCallback(() => {
    setOpen(true);
    if (payload || loading) return;
    setLoading(true);
    setError(null);
    api
      .analyticsTrajectoryExport(id)
      .then((p) => setPayload(p))
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Failed to load analytics export'))
      .finally(() => setLoading(false));
  }, [id, loading, payload]);

  const rawJson = useMemo(() => (payload ? JSON.stringify(payload, null, 2) : ''), [payload]);

  const copyJson = useCallback(() => {
    if (!rawJson) return;
    void navigator.clipboard.writeText(rawJson).then(() => {
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1500);
    });
  }, [rawJson]);

  if (!open) {
    return (
      <button type="button" className="settings-inline-btn" onClick={load}>
        View analytics export
      </button>
    );
  }

  return (
    <section className="usage-export">
      <div className="usage-export__head">
        <div>
          <h4>Analytics export preview</h4>
          {payload && <span className="usage-card__hint">{payload.client} · {payload.source}</span>}
        </div>
        <button type="button" className="settings-inline-btn" onClick={() => setOpen(false)}>
          Hide export
        </button>
      </div>
      {loading && <div className="settings-section__hint">Loading analytics export…</div>}
      {error && <div className="settings-section__error">{error}</div>}
      {payload && (
        <>
          <div className="usage-export__summary">
            <div><span>Session</span><strong>{payload.session.session_id}</strong></div>
            <div><span>Branch</span><strong>{payload.session.branch ?? 'none'}</strong></div>
            <div><span>Task</span><strong>{payload.session.task_title ?? payload.session.task_id ?? 'none'}</strong></div>
            <div><span>Turns</span><strong>{payload.session.turns.length}</strong></div>
            <div><span>Tool calls</span><strong>{payload.session.tool_calls.length}</strong></div>
            <div><span>Last seen</span><strong>{fmtDateTime(payload.session.last_seen_at)}</strong></div>
          </div>
          <div className="usage-export__badges" aria-label="Analytics fidelity">
            {Object.entries(payload.session.fidelity).map(([key, value]) => (
              <span key={key} className={`usage-export__badge usage-export__badge--${value}`}>
                {key.replaceAll('_', ' ')}: {value}
              </span>
            ))}
          </div>
          <div className="usage-export__tables">
            <div>
              <div className="usage-card__hint">Turns</div>
              <div className="usage-mini-table usage-mini-table--turns">
                <div className="usage-mini-table__row usage-mini-table__head">
                  <span>#</span><span>Conversation</span><span>Model</span><span>Tokens</span><span>Cost</span><span>First byte</span>
                </div>
                {payload.session.turns.slice(0, 8).map((turn, idx) => (
                  <div key={turn.turn_usage_id} className="usage-mini-table__row">
                    <span>{idx + 1}</span>
                    <span title={turn.conversation_id}>{turn.conversation_id === payload.session.session_id ? 'root' : turn.conversation_id}</span>
                    <span title={turn.model}>{turn.model}</span>
                    <span className="num">{fmtTokens(totalAnalyticsTokens(turn.tokens))}</span>
                    <span className="num">{turn.cost.pricing_known ? fmtUsd(turn.cost.total_usd ?? 0) : 'unknown'}</span>
                    <span className="num">{fmtLatency(turn.first_byte_latency_ms)}</span>
                  </div>
                ))}
              </div>
              {payload.session.turns.length > 8 && <div className="usage-card__hint">Showing first 8 of {payload.session.turns.length} turns.</div>}
            </div>
            <div>
              <div className="usage-card__hint">Tool calls</div>
              <div className="usage-mini-table usage-mini-table--tools">
                <div className="usage-mini-table__row usage-mini-table__head">
                  <span>Tool</span><span>Result</span><span>Error</span><span>Denied</span><span>Duration</span>
                </div>
                {payload.session.tool_calls.slice(0, 8).map((tool) => (
                  <div key={`${tool.assistant_message_id}:${tool.tool_use_id}`} className="usage-mini-table__row">
                    <span title={tool.tool_name}>{tool.tool_name}</span>
                    <span>{tool.tool_result_message_id ? 'yes' : 'no'}</span>
                    <span>{tool.is_error ? 'yes' : 'no'}</span>
                    <span>{tool.denied ? 'yes' : 'no'}</span>
                    <span className="num">{fmtLatency(tool.duration_ms)}</span>
                  </div>
                ))}
              </div>
              {payload.session.tool_calls.length > 8 && <div className="usage-card__hint">Showing first 8 of {payload.session.tool_calls.length} tool calls.</div>}
            </div>
          </div>
          <details className="usage-export__raw">
            <summary>Raw export JSON</summary>
            <button type="button" className="settings-inline-btn" onClick={copyJson}>{copied ? 'Copied' : 'Copy JSON'}</button>
            <pre>{rawJson}</pre>
          </details>
        </>
      )}
    </section>
  );
}

function ConversationDrill({ id, label, onClose }: { id: string; label: string; onClose: () => void }) {
  const [detail, setDetail] = useState<ConversationUsageDetail | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    setDetail(null);
    setError(null);
    api
      .usageConversationDetail(id)
      .then((d) => {
        if (!cancelled) setDetail(d);
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : 'Failed to load');
      });
    return () => {
      cancelled = true;
    };
  }, [id]);

  // Cumulative tokens across the turn sequence — the "spend over the run" curve
  // for a single conversation.
  const series = useMemo(() => {
    if (!detail) return [];
    let tokens = 0;
    let cost = 0;
    return detail.turns.map((t) => {
      tokens += t.total_tokens;
      if (t.cost.total_usd !== null) cost += t.cost.total_usd;
      return {
        index: t.index + 1,
        input_tokens: t.input_tokens,
        output_tokens: t.output_tokens,
        cache_write_tokens: t.cache_write_tokens,
        cache_read_tokens: t.cache_read_tokens,
        cumTokens: tokens,
        turnCost: t.cost.total_usd,
        cumCost: cost,
        pricingKnown: t.cost.pricing_known,
      };
    });
  }, [detail]);

  const drillTokenSeries = useMemo(() => {
    const hasCacheWrite = detail?.turns.some((t) => t.cache_write_tokens > 0) ?? false;
    return TOKEN_SERIES.filter((s) => s.key !== 'cache_write_tokens' || hasCacheWrite);
  }, [detail]);

  const firstByteRows = useMemo(() => detail?.turns.filter((t) => t.first_byte_at !== null) ?? [], [detail]);

  return (
    <div className="usage-drill">
      <div className="usage-drill__head">
        <div>
          <h3>{label}</h3>
          {detail && (
            <span className="usage-card__hint">
              {fmtTokens(detail.totals.total_tokens)} tokens · {Math.round(detail.totals.turns)} turns ·{' '}
              {fmtCostSummary(detail.totals.cost)} estimated
            </span>
          )}
        </div>
        <button type="button" className="settings-inline-btn" onClick={onClose}>
          Close
        </button>
      </div>
      {error && <div className="settings-section__error">{error}</div>}
      {!detail && !error && <div className="settings-section__hint">Loading…</div>}
      {detail && detail.totals.cost.unknown_turns > 0 && (
        <div className="settings-section__hint">
          Cost estimate excludes {Math.round(detail.totals.cost.unknown_turns)} turns with unknown pricing.
        </div>
      )}
      {detail && firstByteRows.length > 0 && (
        <div className="settings-section__hint">
          First-byte latency observed for {firstByteRows.length} turn{firstByteRows.length === 1 ? '' : 's'}; latest is{' '}
          {fmtLatency(firstByteRows[firstByteRows.length - 1]?.first_byte_latency_ms)}. Historical or unanchored rows render as
          unavailable.
        </div>
      )}
      {detail && <AnalyticsExportPreview id={id} />}
      {detail && series.length > 0 && (
        <div className="usage-drill__charts">
          <div>
            <div className="usage-card__hint">Tokens per turn</div>
            <ResponsiveContainer width="100%" height={180}>
              <BarChart data={series} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                <CartesianGrid stroke={GRID} vertical={false} />
                <XAxis dataKey="index" {...AXIS} />
                <YAxis tickFormatter={fmtTokens} width={48} {...AXIS} />
                <Tooltip content={<TokenTooltip />} />
                <Legend wrapperStyle={{ fontSize: 11 }} />
                {drillTokenSeries.map((s) => (
                  <Bar key={s.key} dataKey={s.key} name={s.label} stackId="tokens" fill={s.color} />
                ))}
              </BarChart>
            </ResponsiveContainer>
          </div>
          <div>
            <div className="usage-card__hint">Cumulative tokens</div>
            <ResponsiveContainer width="100%" height={180}>
              <AreaChart data={series} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                <CartesianGrid stroke={GRID} vertical={false} />
                <XAxis dataKey="index" {...AXIS} />
                <YAxis tickFormatter={fmtTokens} width={48} {...AXIS} />
                <Tooltip content={<TokenTooltip />} />
                <Area
                  dataKey="cumTokens"
                  name="Cumulative"
                  stroke="var(--accent-green)"
                  fill="var(--accent-green)"
                  fillOpacity={0.15}
                />
              </AreaChart>
            </ResponsiveContainer>
          </div>
          <div>
            <div className="usage-card__hint">Cumulative estimated cost</div>
            <ResponsiveContainer width="100%" height={180}>
              <LineChart data={series} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                <CartesianGrid stroke={GRID} vertical={false} />
                <XAxis dataKey="index" {...AXIS} />
                <YAxis tickFormatter={(v: number) => fmtUsd(v)} width={56} {...AXIS} />
                <Tooltip content={<CostTooltip />} />
                <Line type="monotone" dataKey="cumCost" name="Estimated cost" stroke="var(--accent-yellow)" dot={false} />
              </LineChart>
            </ResponsiveContainer>
          </div>
        </div>
      )}
    </div>
  );
}

export function UsagePage() {
  const navigate = useNavigate();
  const [data, setData] = useState<UsageOverview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [drill, setDrill] = useState<{ id: string; label: string } | null>(null);

  const load = useCallback(() => {
    setLoading(true);
    api
      .usageOverview()
      .then((d) => {
        setData(d);
        setError(null);
      })
      .catch((e: unknown) => setError(e instanceof Error ? e.message : 'Failed to load usage'))
      .finally(() => setLoading(false));
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const histogram = useMemo(() => {
    if (!data) return [];
    return data.turn_token_histogram.map((b) => ({
      label: b.hi === null ? `${fmtTokens(b.lo)}+` : `${fmtTokens(b.lo)}–${fmtTokens(b.hi)}`,
      count: b.count,
    }));
  }, [data]);

  const cacheTrend = useMemo(() => {
    if (!data) return [];
    return data.daily.map((d) => ({ day: d.day, rate: cacheHitRate(d.totals) * 100 }));
  }, [data]);

  // Cache write is an Anthropic-only billing concept; OpenAI auto-caches with no
  // write count, so the series is structurally zero on OpenAI-only data. Drop it
  // when nothing in scope wrote to cache, rather than charting a flat-zero band.
  const tokenSeries = useMemo(
    () =>
      TOKEN_SERIES.filter(
        (s) => s.key !== 'cache_write_tokens' || (data?.windows.all.cache_write_tokens ?? 0) > 0,
      ),
    [data],
  );

  const empty = data && data.windows.all.turns === 0;

  return (
    <div id="app" className="list-page">
      <main id="main-area">
        <section className="view active">
          <div className="view-header">
            <h2>Usage</h2>
            <div className="view-header-actions">
              <button type="button" className="settings-inline-btn" onClick={load} disabled={loading}>
                {loading ? 'Refreshing…' : 'Refresh'}
              </button>
              <button type="button" className="settings-inline-btn" onClick={() => navigate(-1)}>
                Back
              </button>
            </div>
          </div>

          {error && <div className="settings-section__error">{error}</div>}
          {!data && loading && <div className="settings-section__hint">Loading…</div>}
          {empty && <div className="settings-section__hint">No usage recorded yet.</div>}

          {data && !empty && (
            <>
              <div className="usage-kpis">
                <KpiCard label="Today" totals={data.windows.today} />
                <KpiCard label="7 days" totals={data.windows.week} />
                <KpiCard label="30 days" totals={data.windows.month} />
                <KpiCard label="All time" totals={data.windows.all} />
              </div>

              {drill && (
                <ConversationDrill id={drill.id} label={drill.label} onClose={() => setDrill(null)} />
              )}

              <div className="usage-grid">
                <ChartCard title="Tokens per day" hint="by token class">
                  <ResponsiveContainer width="100%" height={240}>
                    <AreaChart data={data.daily} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                      <CartesianGrid stroke={GRID} vertical={false} />
                      <XAxis dataKey="day" {...AXIS} minTickGap={32} />
                      <YAxis tickFormatter={fmtTokens} width={48} {...AXIS} />
                      <Tooltip content={<TokenTooltip />} />
                      <Legend wrapperStyle={{ fontSize: 11 }} />
                      {tokenSeries.map((s) => (
                        <Area
                          key={s.key}
                          type="monotone"
                          dataKey={`totals.${s.key}`}
                          name={s.label}
                          stackId="t"
                          stroke={s.color}
                          fill={s.color}
                          fillOpacity={0.25}
                        />
                      ))}
                    </AreaChart>
                  </ResponsiveContainer>
                </ChartCard>

                <ChartCard title="Cache hit rate" hint="% of input tokens served from cache">
                  <ResponsiveContainer width="100%" height={240}>
                    <LineChart data={cacheTrend} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                      <CartesianGrid stroke={GRID} vertical={false} />
                      <XAxis dataKey="day" {...AXIS} minTickGap={32} />
                      <YAxis tickFormatter={(v: number) => `${v.toFixed(0)}%`} domain={[0, 100]} width={40} {...AXIS} />
                      <Tooltip
                        formatter={(v: unknown) => `${numOf(v).toFixed(1)}%`}
                        contentStyle={{ background: 'var(--bg-secondary)', border: `1px solid ${GRID}` }}
                      />
                      <Line type="monotone" dataKey="rate" name="Cache" stroke="var(--accent-purple)" dot={false} />
                    </LineChart>
                  </ResponsiveContainer>
                </ChartCard>

                <ChartCard title="Tokens per turn" hint="distribution across all turns">
                  <ResponsiveContainer width="100%" height={240}>
                    <BarChart data={histogram} margin={{ top: 8, right: 8, left: 0, bottom: 0 }}>
                      <CartesianGrid stroke={GRID} vertical={false} />
                      <XAxis dataKey="label" {...AXIS} interval={0} angle={-30} textAnchor="end" height={56} />
                      <YAxis allowDecimals={false} width={40} {...AXIS} />
                      <Tooltip
                        formatter={tipTurns}
                        contentStyle={{ background: 'var(--bg-secondary)', border: `1px solid ${GRID}` }}
                      />
                      <Bar dataKey="count" name="Turns" fill="var(--accent-blue)" />
                    </BarChart>
                  </ResponsiveContainer>
                </ChartCard>

                <ChartCard title="Tokens by model" hint="all token classes">
                  <ResponsiveContainer width="100%" height={Math.max(120, data.by_model.length * 34)}>
                    <BarChart
                      data={data.by_model}
                      layout="vertical"
                      margin={{ top: 4, right: 16, left: 8, bottom: 4 }}
                    >
                      <CartesianGrid stroke={GRID} horizontal={false} />
                      <XAxis type="number" tickFormatter={fmtTokens} {...AXIS} />
                      <YAxis type="category" dataKey="model" width={130} {...AXIS} />
                      <Tooltip content={<TokenTooltip />} />
                      <Bar dataKey="totals.total_tokens" name="Tokens" fill="var(--accent-green)" />
                    </BarChart>
                  </ResponsiveContainer>
                </ChartCard>

                <ChartCard title="Tokens by provider" hint="all token classes">
                  <ResponsiveContainer width="100%" height={Math.max(120, data.by_provider.length * 40)}>
                    <BarChart
                      data={data.by_provider}
                      layout="vertical"
                      margin={{ top: 4, right: 16, left: 8, bottom: 4 }}
                    >
                      <CartesianGrid stroke={GRID} horizontal={false} />
                      <XAxis type="number" tickFormatter={fmtTokens} {...AXIS} />
                      <YAxis type="category" dataKey="provider" width={90} {...AXIS} />
                      <Tooltip content={<TokenTooltip />} />
                      <Bar dataKey="totals.total_tokens" name="Tokens" fill="var(--accent-blue)" />
                    </BarChart>
                  </ResponsiveContainer>
                </ChartCard>
              </div>

              <ChartCard title="Conversations" hint="highest token use first — click to drill in">
                <div className="usage-table">
                  <div className="usage-table__head usage-table__row">
                    <span>Conversation</span>
                    <span className="num">Tokens</span>
                    <span className="num">Est. cost</span>
                    <span className="num">Turns</span>
                    <span className="num">Cache</span>
                  </div>
                  {data.conversations.map((c) => (
                    <button
                      key={c.root_conversation_id}
                      type="button"
                      className="usage-table__row usage-table__rowbtn"
                      onClick={() => setDrill({ id: c.root_conversation_id, label: c.label })}
                    >
                      <span className="usage-conv-label">
                        <span className="usage-conv-title">{c.label}</span>
                        {c.worktree && <span className="usage-conv-meta">{c.worktree}</span>}
                      </span>
                      <span className="num">{fmtTokens(c.totals.total_tokens)}</span>
                      <span className="num">{fmtCostSummary(c.totals.cost)}</span>
                      <span className="num">{Math.round(c.totals.turns)}</span>
                      <span className="num">{fmtPct(cacheHitRate(c.totals))}</span>
                    </button>
                  ))}
                </div>
              </ChartCard>
            </>
          )}
        </section>
      </main>
    </div>
  );
}
