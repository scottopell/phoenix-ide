export const phases = ['drag', 'hold', 'release_settle', 'post_release'];
export function inspectTimeline(timeline, tolerance) {
  const maxima = Object.fromEntries(phases.map(p => [p, 0]));
  const failures = [];
  for (const frame of timeline) {
    if (!phases.includes(frame.phase)) continue;
    if (frame.missing) { failures.push({ kind: 'observability', t: frame.t, reason: frame.missing }); continue; }
    if (![frame.x, frame.y, frame.expectedX, frame.expectedY].every(Number.isFinite)) {
      failures.push({ kind: 'observability', t: frame.t, reason: 'non-finite geometry' }); continue;
    }
    const distance = Math.hypot(frame.x - frame.expectedX, frame.y - frame.expectedY);
    maxima[frame.phase] = Math.max(maxima[frame.phase], distance);
    if (distance > tolerance) failures.push({ kind: 'displacement', t: frame.t, phase: frame.phase, distance });
  }
  for (const phase of phases) if (!timeline.some(f => f.phase === phase)) failures.push({ kind: 'observability', reason: `no ${phase} samples` });
  return { ok: failures.length === 0, maxima, failures: failures.slice(0, 64), failureCount: failures.length };
}
export function calibrate(timeline) {
  const baseline = inspectTimeline(timeline, 6);
  if (!baseline.ok) throw new Error('No-stream calibration failed; do not expand tolerance');
  return Math.max(2, Math.ceil(Math.max(...Object.values(baseline.maxima))) + 2);
}
export function expectedDetection(name, result, trace, tolerance) {
  const controls = ['oracle-anchor-invalid', 'uncompensated-above-height', 'forced-scroll-bottom'];
  if (!controls.includes(name)) return result.ok;
  if (!trace || !Number.isFinite(tolerance)) return false;
  const injections = trace.events.filter(e => e.kind === 'control-injection' && e.name === name);
  const acknowledgements = trace.events.filter(e => e.kind === 'control-ack' && e.name === name);
  if (injections.length !== 1 || acknowledgements.length !== 1) return false;
  const injection = injections[0], ack = acknowledgements[0];
  if (injection.phase !== 'hold' || ack.phase !== 'hold' || ack.t < injection.t || ack.t - injection.t > 200 || ack.fromFrame !== injection.fromFrame) return false;
  const before = trace.frames.slice(0, injection.fromFrame).filter(f => phases.includes(f.phase));
  const causal = trace.frames.slice(injection.fromFrame, ack.toFrame);
  if (before.length < 2 || causal.length < 1 || causal.length > 4) return false;
  const check = frame => {
    if (frame.missing) return { kind: 'observability', reason: frame.missing };
    if (![frame.x, frame.y, frame.expectedX, frame.expectedY].every(Number.isFinite)) return { kind: 'observability', reason: 'non-finite geometry' };
    return Math.hypot(frame.x - frame.expectedX, frame.y - frame.expectedY) > tolerance ? { kind: 'displacement' } : null;
  };
  if (before.some(check) || causal.some(f => f.t < injection.t || f.t > ack.t || f.phase !== 'hold')) return false;
  // Only injection→ack can license detection, never a later scheduled reflow.
  if (trace.events.some(e => e.kind === 'schedule' && e.type !== 'control' && e.t >= injection.t && e.t <= ack.t)) return false;
  return causal.map(check).some(f => f && (name === 'oracle-anchor-invalid'
    ? f.reason === 'deliberate-oracle-anchor-invalid'
    : name === 'uncompensated-above-height' ? f.kind === 'displacement'
      : f.kind === 'displacement' || f.reason === 'exact-occurrence-count:0'));
}
