import { useState, useEffect } from 'react';

/**
 * Debug component to test various keyboard detection methods on iOS.
 * Takes over entire page with its own styles to avoid CSS conflicts.
 */
export function KeyboardDebug() {
  const [metrics, setMetrics] = useState(getMetrics());
  const [inputFocused, setInputFocused] = useState(false);

  // Override problematic CSS on mount
  useEffect(() => {
    // Remove position:fixed from html/body that breaks iOS
    document.documentElement.style.position = 'static';
    document.documentElement.style.height = 'auto';
    document.documentElement.style.overflow = 'auto';
    document.body.style.position = 'static';
    document.body.style.height = 'auto';
    document.body.style.overflow = 'auto';
    
    return () => {
      document.documentElement.style.position = '';
      document.documentElement.style.height = '';
      document.documentElement.style.overflow = '';
      document.body.style.position = '';
      document.body.style.height = '';
      document.body.style.overflow = '';
    };
  }, []);

  useEffect(() => {
    const update = () => setMetrics(getMetrics());
    
    const interval = setInterval(update, 100);
    
    const vv = window.visualViewport;
    if (vv) {
      vv.addEventListener('resize', update);
      vv.addEventListener('scroll', update);
    }
    window.addEventListener('resize', update);
    
    // Track focus state
    const onFocusIn = (e: FocusEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        setInputFocused(true);
      }
    };
    const onFocusOut = (e: FocusEvent) => {
      if (e.target instanceof HTMLInputElement || e.target instanceof HTMLTextAreaElement) {
        setInputFocused(false);
      }
    };
    document.addEventListener('focusin', onFocusIn);
    document.addEventListener('focusout', onFocusOut);

    return () => {
      clearInterval(interval);
      if (vv) {
        vv.removeEventListener('resize', update);
        vv.removeEventListener('scroll', update);
      }
      window.removeEventListener('resize', update);
      document.removeEventListener('focusin', onFocusIn);
      document.removeEventListener('focusout', onFocusOut);
    };
  }, []);

  const m = metrics;
  
  // Different keyboard detection algorithms
  const vvHeightDiff = m.windowInnerHeight - m.vvHeight;
  const vvHeightRatio = m.vvHeight / m.windowInnerHeight;
  
  const detection = {
    diff100: vvHeightDiff > 100,
    diff150: vvHeightDiff > 150,
    diff200: vvHeightDiff > 200,
    ratio80: vvHeightRatio < 0.8,
    ratio70: vvHeightRatio < 0.7,
    focused: inputFocused,
    combo: inputFocused && vvHeightDiff > 100,
  };

  const Row = ({ label, value, bool }: { label: string; value: string | number; bool?: boolean }) => (
    <div style={{ 
      display: 'flex', 
      justifyContent: 'space-between',
      padding: '4px 0',
      fontSize: '16px',
      color: bool === undefined ? '#0f0' : (bool ? '#0f0' : '#f00'),
    }}>
      <span>{label}</span>
      <span style={{ fontWeight: 'bold' }}>{String(value)}</span>
    </div>
  );

  return (
    <div style={{
      position: 'fixed',
      top: 0,
      left: 0,
      right: 0,
      bottom: 0,
      background: '#000',
      color: '#0f0',
      fontFamily: 'monospace',
      padding: '16px',
      zIndex: 99999,
      display: 'flex',
      flexDirection: 'column',
      boxSizing: 'border-box',
    }}>
      {/* INPUT AT VERY TOP */}
      <input 
        type="text" 
        placeholder="TAP HERE TO TEST KEYBOARD"
        style={{
          width: '100%',
          padding: '20px',
          fontSize: '18px',
          background: '#222',
          color: '#fff',
          border: '3px solid #0f0',
          borderRadius: '8px',
          marginBottom: '16px',
          flexShrink: 0,
          boxSizing: 'border-box',
        }}
      />
      
      {/* METRICS - scrollable */}
      <div style={{ flex: 1, overflow: 'auto' }}>
        <div style={{ color: '#ff0', fontWeight: 'bold', marginBottom: '8px', fontSize: '18px' }}>RAW VALUES</div>
        <Row label="window.innerHeight" value={m.windowInnerHeight} />
        <Row label="visualViewport.height" value={m.vvHeight} />
        <Row label="vv.offsetTop" value={m.vvOffsetTop} />
        <Row label="vv.scale" value={m.vvScale} />
        <Row label="window.scrollY" value={m.windowScrollY} />
        <Row label="activeElement" value={m.activeElement} />
        
        <div style={{ color: '#ff0', fontWeight: 'bold', margin: '16px 0 8px', fontSize: '18px' }}>
          DETECTION (diff={vvHeightDiff}px)
        </div>
        <Row label="diff > 100px" value={detection.diff100} bool={detection.diff100} />
        <Row label="diff > 150px" value={detection.diff150} bool={detection.diff150} />
        <Row label="diff > 200px" value={detection.diff200} bool={detection.diff200} />
        <Row label="ratio < 0.8" value={detection.ratio80} bool={detection.ratio80} />
        <Row label="ratio < 0.7" value={detection.ratio70} bool={detection.ratio70} />
        <Row label="inputFocused" value={detection.focused} bool={detection.focused} />
        <Row label="focused + diff>100" value={detection.combo} bool={detection.combo} />
      </div>
    </div>
  );
}

function getMetrics() {
  const vv = window.visualViewport;
  return {
    windowInnerHeight: window.innerHeight,
    windowInnerWidth: window.innerWidth,
    windowScrollY: Math.round(window.scrollY),
    vvHeight: vv ? Math.round(vv.height) : 0,
    vvWidth: vv ? Math.round(vv.width) : 0,
    vvOffsetTop: vv ? Math.round(vv.offsetTop) : 0,
    vvScale: vv ? vv.scale.toFixed(2) : 'N/A',
    activeElement: document.activeElement?.tagName || 'null',
  };
}
