// Performance monitoring utilities

export interface PerformanceMetrics {
  cacheHits: number;
  cacheMisses: number;
  networkRequests: number;
  avgResponseTime: number;
  cacheHitRate: number;
}

class PerformanceMonitor {
  private metrics = {
    cacheHits: 0,
    cacheMisses: 0,
    networkRequests: 0,
    responseTimes: [] as number[],
  };

  recordCacheHit(source: 'memory' | 'indexeddb') {
    this.metrics.cacheHits++;
    if (typeof window !== 'undefined' && window.performance) {
      window.performance.mark(`cache-hit-${source}`);
    }
  }

  recordCacheMiss() {
    this.metrics.cacheMisses++;
  }

  recordNetworkRequest(duration: number) {
    this.metrics.networkRequests++;
    this.metrics.responseTimes.push(duration);
    
    // Keep only last 100 response times
    if (this.metrics.responseTimes.length > 100) {
      this.metrics.responseTimes = this.metrics.responseTimes.slice(-100);
    }
  }

  getMetrics(): PerformanceMetrics {
    const total = this.metrics.cacheHits + this.metrics.cacheMisses;
    const avgResponseTime = this.metrics.responseTimes.length > 0
      ? this.metrics.responseTimes.reduce((a, b) => a + b, 0) / this.metrics.responseTimes.length
      : 0;

    return {
      cacheHits: this.metrics.cacheHits,
      cacheMisses: this.metrics.cacheMisses,
      networkRequests: this.metrics.networkRequests,
      avgResponseTime: Math.round(avgResponseTime),
      cacheHitRate: total > 0 ? this.metrics.cacheHits / total : 0,
    };
  }

  logMetrics() {
    const metrics = this.getMetrics();
    console.log('Phoenix IDE Performance Metrics:', {
      ...metrics,
      cacheHitRate: `${(metrics.cacheHitRate * 100).toFixed(1)}%`,
      avgResponseTime: `${metrics.avgResponseTime}ms`,
    });
  }

  reset() {
    this.metrics = {
      cacheHits: 0,
      cacheMisses: 0,
      networkRequests: 0,
      responseTimes: [],
    };
  }
}

export const performanceMonitor = new PerformanceMonitor();

// Log metrics every 5 minutes in development
if (import.meta.env.DEV) {
  setInterval(() => {
    performanceMonitor.logMetrics();
  }, 5 * 60 * 1000);
}
