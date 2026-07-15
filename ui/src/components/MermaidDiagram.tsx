import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { Maximize2 } from 'lucide-react';
import type { Mermaid } from 'mermaid';
import { useTheme, type Theme } from '../hooks/useTheme';
import { CopyButton } from './CopyButton';

type MermaidDiagramProps = {
  code: string;
  className?: string;
};

type RenderState =
  | { status: 'rendering' }
  | { status: 'rendered'; svg: string }
  | { status: 'error'; message: string };

function mermaidThemeVariables(theme: Theme) {
  if (theme === 'light') {
    return {
      background: '#ffffff',
      mainBkg: '#f6f8fa',
      secondBkg: '#eef6ff',
      primaryColor: '#eef6ff',
      primaryTextColor: '#24292f',
      primaryBorderColor: '#0969da',
      secondaryColor: '#f6f8fa',
      secondaryTextColor: '#24292f',
      secondaryBorderColor: '#d0d7de',
      tertiaryColor: '#fff8c5',
      tertiaryTextColor: '#24292f',
      tertiaryBorderColor: '#9a6700',
      clusterBkg: '#f6f8fa',
      clusterBorder: '#d0d7de',
      lineColor: '#57606a',
      textColor: '#24292f',
      fontFamily: 'Inter, ui-sans-serif, system-ui, sans-serif',
      noteBkgColor: '#fff8c5',
      noteTextColor: '#24292f',
      noteBorderColor: '#d0d7de',
    };
  }

  return {
    background: '#0d1117',
    mainBkg: '#161b22',
    secondBkg: '#1f2a37',
    primaryColor: '#1f2a37',
    primaryTextColor: '#e6edf3',
    primaryBorderColor: '#58a6ff',
    secondaryColor: '#21262d',
    secondaryTextColor: '#e6edf3',
    secondaryBorderColor: '#30363d',
    tertiaryColor: '#2d2412',
    tertiaryTextColor: '#e6edf3',
    tertiaryBorderColor: '#d29922',
    clusterBkg: '#161b22',
    clusterBorder: '#30363d',
    lineColor: '#8b949e',
    textColor: '#e6edf3',
    fontFamily: 'Inter, ui-sans-serif, system-ui, sans-serif',
    noteBkgColor: '#2d2412',
    noteTextColor: '#e6edf3',
    noteBorderColor: '#30363d',
  };
}

function standaloneSvg(svg: string, background: string): string {
  return svg.replace(
    /(<svg\b[^>]*>)/,
    `$1<rect width="100%" height="100%" fill="${background}"/>`,
  );
}

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message;
  if (typeof error === 'string' && error.trim()) return error;
  return 'Mermaid could not render this diagram.';
}

export function MermaidDiagram({ code, className = '' }: MermaidDiagramProps) {
  const { theme } = useTheme();
  const reactId = useId();
  const diagramId = useMemo(
    () => `phoenix-mermaid-${reactId.replace(/[^a-zA-Z0-9_-]/g, '')}`,
    [reactId],
  );
  const source = code.replace(/\n$/, '');
  const [mode, setMode] = useState<'diagram' | 'source'>('diagram');
  const [renderState, setRenderState] = useState<RenderState>({ status: 'rendering' });
  const svgContainerRef = useRef<HTMLDivElement | null>(null);
  const figureRef = useRef<HTMLElement | null>(null);
  const standaloneUrlRef = useRef<string | null>(null);

  useEffect(() => {
    const wrapper = figureRef.current?.closest('pre');
    wrapper?.classList.add('mermaid-diagram-pre');
    return () => wrapper?.classList.remove('mermaid-diagram-pre');
  }, []);

  useEffect(() => {
    let cancelled = false;
    const themeVariables = mermaidThemeVariables(theme);
    if (standaloneUrlRef.current) {
      URL.revokeObjectURL(standaloneUrlRef.current);
      standaloneUrlRef.current = null;
    }
    setRenderState({ status: 'rendering' });

    // mermaid.render injects a temporary measuring node into <body> with id
    // `d${diagramId}`. It removes it on success, but on a syntax error it throws
    // before cleanup, orphaning an error-diagram SVG that inflates page height.
    const removeOrphanedNode = () => document.getElementById(`d${diagramId}`)?.remove();

    import('mermaid')
      .then(({ default: mermaid }: { default: Mermaid }) => {
        if (cancelled) return;
        mermaid.initialize({
          startOnLoad: false,
          theme: 'base',
          securityLevel: 'strict',
          themeVariables,
          flowchart: {
            curve: 'basis',
            htmlLabels: false,
            nodeSpacing: 48,
            rankSpacing: 58,
            useMaxWidth: true,
          },
        });
        return mermaid.render(diagramId, source);
      })
      .then((result) => {
        if (cancelled || !result) return;
        const { svg, bindFunctions } = result;
        standaloneUrlRef.current = URL.createObjectURL(
          new Blob([standaloneSvg(svg, themeVariables.background)], { type: 'image/svg+xml' }),
        );
        setRenderState({ status: 'rendered', svg });
        window.requestAnimationFrame(() => {
          if (cancelled || !bindFunctions || !svgContainerRef.current) return;
          bindFunctions(svgContainerRef.current);
        });
      })
      .catch((error: unknown) => {
        removeOrphanedNode();
        if (!cancelled) setRenderState({ status: 'error', message: errorMessage(error) });
      });

    return () => {
      cancelled = true;
      removeOrphanedNode();
      if (standaloneUrlRef.current) {
        URL.revokeObjectURL(standaloneUrlRef.current);
        standaloneUrlRef.current = null;
      }
    };
  }, [diagramId, source, theme]);

  const showSource = mode === 'source' || renderState.status === 'error';

  return (
    <figure ref={figureRef} className={`mermaid-diagram ${className}`.trim()} data-testid="mermaid-diagram">
      <div className="mermaid-diagram-toolbar">
        <div className="mermaid-diagram-tabs" role="group" aria-label="Mermaid view mode">
          <button
            type="button"
            className={`mermaid-diagram-tab ${mode === 'diagram' ? 'active' : ''}`}
            onClick={() => setMode('diagram')}
            disabled={renderState.status === 'error'}
          >
            Diagram
          </button>
          <button
            type="button"
            className={`mermaid-diagram-tab ${mode === 'source' ? 'active' : ''}`}
            onClick={() => setMode('source')}
          >
            Source
          </button>
        </div>
        <div className="mermaid-diagram-actions">
          {mode === 'diagram' && renderState.status === 'rendered' && standaloneUrlRef.current && (
            <a
              className="mermaid-fullscreen"
              href={standaloneUrlRef.current}
              target="_blank"
              rel="noopener noreferrer"
              aria-label="Open Mermaid diagram fullscreen"
              title="Open fullscreen"
            >
              <Maximize2 size={16} aria-hidden="true" />
            </a>
          )}
          <CopyButton text={source} className="mermaid-copy-source" title="Copy Mermaid source" />
        </div>
      </div>

      <div className="mermaid-diagram-body">
        {renderState.status === 'error' && (
          <div className="mermaid-diagram-error" role="alert">
            <strong>Mermaid render failed.</strong>
            <span>{renderState.message}</span>
          </div>
        )}

        {showSource ? (
          <pre className="mermaid-source"><code className="mermaid-source-code">{source}</code></pre>
        ) : renderState.status === 'rendered' ? (
          <div
            id={`${diagramId}-container`}
            ref={svgContainerRef}
            className="mermaid-svg"
            dangerouslySetInnerHTML={{ __html: renderState.svg }}
          />
        ) : (
          <div className="mermaid-diagram-loading" aria-live="polite">Rendering diagram…</div>
        )}
      </div>
    </figure>
  );
}