import { useState, useEffect } from 'react';
import { useSettings } from '../hooks';

interface LayoutMetrics {
  // Window
  windowInnerWidth: number;
  windowInnerHeight: number;
  windowScrollY: number;
  
  // Visual Viewport
  vvHeight: number | null;
  vvOffsetTop: number | null;
  
  // #app container
  appHeight: number | null;
  appOffsetTop: number | null;
  appBoundingTop: number | null;
  
  // #main-area scroll container
  mainScrollTop: number | null;
  mainScrollHeight: number | null;
  mainClientHeight: number | null;
  
  // #state-bar
  stateBarBoundingTop: number | null;
  
  // Computed
  keyboardHeight: number;
}

function getMetrics(): LayoutMetrics {
  const vv = window.visualViewport;
  const app = document.getElementById('app');
  const main = document.getElementById('main-area');
  const stateBar = document.getElementById('state-bar');
  
  return {
    windowInnerWidth: window.innerWidth,
    windowInnerHeight: window.innerHeight,
    windowScrollY: window.scrollY,
    
    vvHeight: vv?.height ?? null,
    vvOffsetTop: vv?.offsetTop ?? null,
    
    appHeight: app?.clientHeight ?? null,
    appOffsetTop: app?.offsetTop ?? null,
    appBoundingTop: app?.getBoundingClientRect().top ?? null,
    
    mainScrollTop: main?.scrollTop ?? null,
    mainScrollHeight: main?.scrollHeight ?? null,
    mainClientHeight: main?.clientHeight ?? null,
    
    stateBarBoundingTop: stateBar?.getBoundingClientRect().top ?? null,
    
    keyboardHeight: vv ? window.innerHeight - vv.height : 0,
  };
}

export function LayoutDebugOverlay() {
  const { settings } = useSettings();
  const [metrics, setMetrics] = useState<LayoutMetrics>(getMetrics);

  useEffect(() => {
    if (!settings.showLayoutOverlay) return;

    const update = () => setMetrics(getMetrics());
    
    // Update on various events
    const interval = setInterval(update, 200);
    window.addEventListener('scroll', update);
    window.addEventListener('resize', update);
    document.addEventListener('scroll', update);
    
    const vv = window.visualViewport;
    if (vv) {
      vv.addEventListener('resize', update);
      vv.addEventListener('scroll', update);
    }

    return () => {
      clearInterval(interval);
      window.removeEventListener('scroll', update);
      window.removeEventListener('resize', update);
      document.removeEventListener('scroll', update);
      if (vv) {
        vv.removeEventListener('resize', update);
        vv.removeEventListener('scroll', update);
      }
    };
  }, [settings.showLayoutOverlay]);

  if (!settings.showLayoutOverlay) return null;

  const m = metrics;
  
  return (
    <div className="layout-debug-overlay">
      <div className="debug-section">
        <div className="debug-title">Window</div>
        <div>{m.windowInnerWidth}×{m.windowInnerHeight}</div>
        <div>scrollY: {m.windowScrollY}</div>
      </div>
      
      <div className="debug-section">
        <div className="debug-title">VisualVP</div>
        <div>h: {m.vvHeight?.toFixed(0) ?? 'N/A'}</div>
        <div>offTop: {m.vvOffsetTop?.toFixed(0) ?? 'N/A'}</div>
        <div>kbH: {m.keyboardHeight.toFixed(0)}</div>
      </div>
      
      <div className="debug-section">
        <div className="debug-title">#app</div>
        <div>h: {m.appHeight}</div>
        <div>offTop: {m.appOffsetTop}</div>
        <div>boundTop: {m.appBoundingTop?.toFixed(0)}</div>
      </div>
      
      <div className="debug-section">
        <div className="debug-title">#main-area</div>
        <div>scrollTop: {m.mainScrollTop?.toFixed(0)}</div>
        <div>scrollH: {m.mainScrollHeight}</div>
        <div>clientH: {m.mainClientHeight}</div>
      </div>
      
      <div className="debug-section highlight">
        <div className="debug-title">#state-bar</div>
        <div>boundTop: {m.stateBarBoundingTop?.toFixed(0)}</div>
      </div>
    </div>
  );
}
