import { useState, useEffect } from 'react';
import { performanceMonitor } from '../performance';
import type { PerformanceMetrics } from '../performance';

export function PerformanceDashboard() {
  const [metrics, setMetrics] = useState<PerformanceMetrics | null>(null);
  const [visible, setVisible] = useState(false);
  
  useEffect(() => {
    // Check if dev mode from URL param
    const params = new URLSearchParams(window.location.search);
    if (params.get('debug') === '1' || import.meta.env.DEV) {
      setVisible(true);
    }
    
    // Update metrics every second when visible
    if (!visible) return;
    
    const updateMetrics = () => {
      setMetrics(performanceMonitor.getMetrics());
    };
    
    updateMetrics();
    const interval = setInterval(updateMetrics, 1000);
    
    return () => clearInterval(interval);
  }, [visible]);
  
  if (!visible || !metrics) return null;
  
  return (
    <div className="performance-dashboard">
      <h4>Performance Metrics</h4>
      <div className="metric-grid">
        <div className="metric">
          <span className="label">Cache Hit Rate</span>
          <span className="value">{(metrics.cacheHitRate * 100).toFixed(1)}%</span>
        </div>
        <div className="metric">
          <span className="label">Cache Hits</span>
          <span className="value">{metrics.cacheHits}</span>
        </div>
        <div className="metric">
          <span className="label">Network Requests</span>
          <span className="value">{metrics.networkRequests}</span>
        </div>
        <div className="metric">
          <span className="label">Avg Response Time</span>
          <span className="value">{metrics.avgResponseTime}ms</span>
        </div>
      </div>
      <button 
        className="reset-btn"
        onClick={() => {
          performanceMonitor.reset();
          setMetrics(performanceMonitor.getMetrics());
        }}
      >
        Reset
      </button>
    </div>
  );
}
