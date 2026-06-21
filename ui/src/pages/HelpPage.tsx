import { useEffect, useMemo, useState } from 'react';
import { useSearchParams } from 'react-router-dom';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, oneDark, oneLight } from '../utils/syntaxHighlighter';
import { useTheme } from '../hooks/useTheme';
import './HelpPage.css';

// The user guide lives in docs/guide/, embedded into the binary and served at
// /api/help/<path>. This page fetches SUMMARY.md to build the nav, fetches each
// page on navigation, strips its frontmatter, and rewrites in-doc .md links to
// stay inside /help (the dual-render contract: the same markdown renders on
// GitHub and here).

const REMARK_PLUGINS = [remarkGfm];
const DEFAULT_PAGE = 'README.md';

interface NavEntry {
  title: string;
  path: string | null; // null = a planned (unwritten) page; shown disabled
}
interface NavSection {
  label: string;
  entries: NavEntry[];
}

/** Parse SUMMARY.md (the manifest) into nav sections. Linked `- [t](p.md)`
 *  entries are navigable; everything else under a heading is planned. */
function parseSummary(md: string): NavSection[] {
  const sections: NavSection[] = [];
  let current: NavSection | null = null;
  for (const raw of md.split('\n')) {
    const line = raw.trimEnd();
    const heading = /^#\s+(.+)/.exec(line);
    if (heading) {
      const label = (heading[1] ?? '').trim();
      if (label.toLowerCase() === 'summary') continue;
      current = { label, entries: [] };
      sections.push(current);
      continue;
    }
    const item = /^\s*-\s+(.*)$/.exec(line);
    if (!item) continue;
    const itemText = item[1] ?? '';
    if (!current) {
      current = { label: '', entries: [] };
      sections.push(current);
    }
    const linked = /\[([^\]]+)\]\(([^)]+)\)/.exec(itemText);
    const linkPath = linked?.[2] ?? '';
    if (linked && linkPath.endsWith('.md')) {
      current.entries.push({ title: (linked[1] ?? '').trim(), path: linkPath.trim() });
    } else {
      const title = itemText.replace(/\s+—.*$/, '').replace(/[`*]/g, '').trim();
      if (title) current.entries.push({ title, path: null });
    }
  }
  return sections.filter((s) => s.entries.length > 0);
}

function stripFrontmatter(md: string): string {
  const m = /^---\n[\s\S]*?\n---\n?/.exec(md);
  return m ? md.slice(m[0].length) : md;
}

/** Resolve a relative in-doc href against the current page's directory, to a
 *  doc-root-relative path (e.g. modes.md + "../reference/glossary.md"). */
function resolveDocPath(current: string, href: string): string {
  const baseDir = current.includes('/') ? current.slice(0, current.lastIndexOf('/') + 1) : '';
  try {
    return new URL(href, 'https://x/' + baseDir).pathname.replace(/^\//, '');
  } catch {
    return href;
  }
}

export function HelpPage() {
  const [params, setParams] = useSearchParams();
  const page = params.get('p') || DEFAULT_PAGE;
  const { theme } = useTheme();
  const syntaxStyle = theme === 'light' ? oneLight : oneDark;

  const [nav, setNav] = useState<NavSection[]>([]);
  const [content, setContent] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  // Manifest once.
  useEffect(() => {
    let cancelled = false;
    fetch('/api/help/SUMMARY.md')
      .then((r) => (r.ok ? r.text() : Promise.reject(new Error('manifest'))))
      .then((t) => { if (!cancelled) setNav(parseSummary(t)); })
      .catch(() => { /* nav is optional — the page still renders */ });
    return () => { cancelled = true; };
  }, []);

  // Current page on every navigation.
  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    fetch(`/api/help/${page}`)
      .then((r) => (r.ok ? r.text() : Promise.reject(new Error(`Couldn't load ${page}`))))
      .then((t) => { if (!cancelled) { setContent(stripFrontmatter(t)); setLoading(false); } })
      .catch((e) => { if (!cancelled) { setError(e.message); setLoading(false); } });
    return () => { cancelled = true; };
  }, [page]);

  const components = useMemo(
    () =>
      ({
        a: ({ href, children, ...props }: { href?: string; children?: React.ReactNode; [k: string]: unknown }) => {
          if (!href) return <a {...props}>{children}</a>;
          if (/^(https?:|mailto:)/.test(href)) {
            return <a href={href} target="_blank" rel="noopener noreferrer" {...props}>{children}</a>;
          }
          if (href.startsWith('#')) return <a href={href} {...props}>{children}</a>;
          const resolved = resolveDocPath(page, href).replace(/#.*$/, '');
          return (
            <a
              href={`/help?p=${encodeURIComponent(resolved)}`}
              onClick={(e) => { e.preventDefault(); setParams({ p: resolved }); }}
              {...props}
            >
              {children}
            </a>
          );
        },
        code: ({ inline, className, children, ...props }: { inline?: boolean; className?: string; children?: React.ReactNode; [k: string]: unknown }) => {
          const match = /language-([^\s]+)/.exec(className || '');
          return !inline && match ? (
            <SyntaxHighlighter style={syntaxStyle} language={match[1]} PreTag="div" {...props}>
              {String(children).replace(/\n$/, '')}
            </SyntaxHighlighter>
          ) : (
            <code className={className} {...props}>{children}</code>
          );
        },
      }) as unknown as Components,
    [page, setParams, syntaxStyle],
  );

  return (
    <div className="help-page">
      <nav className="help-nav" aria-label="User guide">
        <a
          className={`help-nav-home${page === DEFAULT_PAGE ? ' help-nav-current' : ''}`}
          href="/help"
          onClick={(e) => { e.preventDefault(); setParams({ p: DEFAULT_PAGE }); }}
        >
          Phoenix User Guide
        </a>
        {nav.map((section) => (
          <div key={section.label} className="help-nav-section">
            {section.label && <div className="help-nav-section-label">{section.label}</div>}
            {section.entries.map((entry) =>
              entry.path ? (
                <a
                  key={entry.path}
                  className={`help-nav-link${entry.path === page ? ' help-nav-current' : ''}`}
                  href={`/help?p=${encodeURIComponent(entry.path)}`}
                  onClick={(e) => { e.preventDefault(); setParams({ p: entry.path! }); }}
                >
                  {entry.title}
                </a>
              ) : (
                <span key={entry.title} className="help-nav-link help-nav-planned">{entry.title}</span>
              ),
            )}
          </div>
        ))}
      </nav>
      <main className="help-content viewer-markdown">
        {loading ? (
          <div className="help-status">Loading…</div>
        ) : error ? (
          <div className="help-status">{error}</div>
        ) : (
          <ReactMarkdown remarkPlugins={REMARK_PLUGINS} components={components}>
            {content}
          </ReactMarkdown>
        )}
      </main>
    </div>
  );
}
