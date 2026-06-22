import { useEffect, useMemo, useState } from 'react';
import { useSearchParams, useNavigate, useLocation } from 'react-router-dom';
import ReactMarkdown from 'react-markdown';
import type { Components } from 'react-markdown';
import remarkGfm from 'remark-gfm';
import { SyntaxHighlighter, oneDark, oneLight } from '../utils/syntaxHighlighter';
import { useTheme } from '../hooks/useTheme';
import './HelpPage.css';

// The user guide lives in docs/guide/, embedded into the binary and served at
// /api/help/<path>. This page fetches SUMMARY.md to build the nav, fetches each
// page on navigation, strips its frontmatter, and rewrites in-doc .md links to
// stay inside /help — preserving #fragments and generating heading anchors so
// section links land where they do on GitHub (the dual-render contract).

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

/** Resolve a relative in-doc path (no fragment) against the current page's
 *  directory, to a doc-root-relative path. */
function resolveDocPath(current: string, relPath: string): string {
  const baseDir = current.includes('/') ? current.slice(0, current.lastIndexOf('/') + 1) : '';
  try {
    return new URL(relPath, 'https://x/' + baseDir).pathname.replace(/^\//, '');
  } catch {
    return relPath;
  }
}

/** GitHub-style heading slug: lowercase, drop punctuation except word/space/-,
 *  spaces → hyphens. Matches the `#fragment` anchors used in the docs. */
function slugify(text: string): string {
  return text.toLowerCase().trim().replace(/[^\w\s-]/g, '').replace(/\s+/g, '-');
}

/** Flatten a React heading's children to plain text for slugging. */
function nodeText(node: React.ReactNode): string {
  if (node == null || node === false || node === true) return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(nodeText).join('');
  const el = node as { props?: { children?: React.ReactNode } };
  return el.props ? nodeText(el.props.children) : '';
}

export function HelpPage() {
  const [params] = useSearchParams();
  const navigate = useNavigate();
  const { hash } = useLocation();
  const page = params.get('p') || DEFAULT_PAGE;
  const { theme } = useTheme();
  const syntaxStyle = theme === 'light' ? oneLight : oneDark;

  const [nav, setNav] = useState<NavSection[]>([]);
  const [content, setContent] = useState('');
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  /** Navigate within the guide, carrying an optional `#fragment`. */
  const go = (path: string, frag = '') => navigate(`/help?p=${encodeURIComponent(path)}${frag}`);

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

  // After content renders, scroll to the #fragment heading (or to the top on a
  // plain page change).
  useEffect(() => {
    if (loading) return;
    const id = hash ? decodeURIComponent(hash.slice(1)) : '';
    const el = id ? document.getElementById(id) : null;
    if (el) el.scrollIntoView();
    else document.querySelector('.help-content')?.scrollTo(0, 0);
  }, [content, hash, loading]);

  const components = useMemo(() => {
    const heading = (Tag: 'h1' | 'h2' | 'h3' | 'h4') =>
      ({ children, ...props }: { children?: React.ReactNode; [k: string]: unknown }) => (
        <Tag id={slugify(nodeText(children))} {...props}>{children}</Tag>
      );
    return {
      h1: heading('h1'),
      h2: heading('h2'),
      h3: heading('h3'),
      h4: heading('h4'),
      a: ({ href, children, ...props }: { href?: string; children?: React.ReactNode; [k: string]: unknown }) => {
        if (!href) return <a {...props}>{children}</a>;
        if (/^(https?:|mailto:)/.test(href)) {
          return <a href={href} target="_blank" rel="noopener noreferrer" {...props}>{children}</a>;
        }
        if (href.startsWith('#')) {
          return <a href={href} onClick={(e) => { e.preventDefault(); go(page, href); }} {...props}>{children}</a>;
        }
        const hashIdx = href.indexOf('#');
        const relPath = hashIdx >= 0 ? href.slice(0, hashIdx) : href;
        const frag = hashIdx >= 0 ? href.slice(hashIdx) : '';
        const resolved = resolveDocPath(page, relPath);
        return (
          <a
            href={`/help?p=${encodeURIComponent(resolved)}${frag}`}
            onClick={(e) => { e.preventDefault(); go(resolved, frag); }}
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
    } as unknown as Components;
  }, [page, syntaxStyle]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="help-page">
      <nav className="help-nav" aria-label="User guide">
        <a
          className={`help-nav-home${page === DEFAULT_PAGE ? ' help-nav-current' : ''}`}
          href="/help"
          onClick={(e) => { e.preventDefault(); go(DEFAULT_PAGE); }}
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
                  onClick={(e) => { e.preventDefault(); go(entry.path!); }}
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
