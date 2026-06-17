import { useLayoutEffect, useRef, useState } from 'react';
import { TerminalPanel } from '../components/TerminalPanel';

// Dedicated full-page mount of the singleton global terminal. Shared-session
// semantics across the `/new` pane and this route are specified in
// specs/terminal REQ-TERM-WS-001 (both target WorkScope::Global).
//
// TerminalPanel is sized in absolute pixels (it drives xterm's FitAddon off the
// `height` prop), so the page measures its own content box and feeds that height
// down, re-measuring on viewport/layout changes via ResizeObserver.
export function TerminalPage() {
  const ref = useRef<HTMLDivElement>(null);
  const [height, setHeight] = useState(0);

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const measure = () => setHeight(el.clientHeight);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  return (
    <div ref={ref} className="terminal-page">
      {height > 0 && (
        <TerminalPanel
          scope={{ kind: 'global' }}
          height={height}
          collapsed={false}
          standalone
          onExpand={() => {}}
          onCollapse={() => {}}
        />
      )}
    </div>
  );
}
