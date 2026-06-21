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
import { api } from '../api';
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
        <span>tokens</span>
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
}

function TokenTooltip({ active, payload, label }: { active?: boolean; payload?: TooltipEntry[]; label?: string }) {
  if (!active || !payload?.length) return null;
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
    return detail.turns.map((t) => {
      tokens += t.total_tokens;
      return { index: t.index + 1, turnTokens: t.total_tokens, cumTokens: tokens };
    });
  }, [detail]);

  return (
    <div className="usage-drill">
      <div className="usage-drill__head">
        <div>
          <h3>{label}</h3>
          {detail && (
            <span className="usage-card__hint">
              {fmtTokens(detail.totals.total_tokens)} tokens · {Math.round(detail.totals.turns)} turns
            </span>
          )}
        </div>
        <button type="button" className="settings-inline-btn" onClick={onClose}>
          Close
        </button>
      </div>
      {error && <div className="settings-section__error">{error}</div>}
      {!detail && !error && <div className="settings-section__hint">Loading…</div>}
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
                <Bar dataKey="turnTokens" name="Tokens" fill="var(--accent-blue)" />
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
